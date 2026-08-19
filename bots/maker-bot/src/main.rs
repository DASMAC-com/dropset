//! `dropset-maker-bot` entrypoint.
//!
//! Default mode supervises every demo market live against a localnet validator:
//! discover the markets, fund the leader, and drive the tick loop, one batched
//! feed poll shared across them. `--dry-run` instead polls the tiered feeds
//! once and prints the reference each market *would* stamp — the wiring check
//! that every venue is reachable and decoding, with no validator and no writes.
//! Pass `--drop <tier>`
//! (repeatable: `pyth`, `coinbase`, `kraken`, `coingecko`, `cmc`, `fx`) in a
//! dry run to suppress a tier and watch the cascade fall through to the next
//! one — dropping `pyth` is how you check the Frankfurter FX fallback still
//! carries the anchor.
//!
//! Flags:
//!   --rpc <url>            RPC endpoint (default http://127.0.0.1:8899)
//!   --ws <url>             PubSub websocket (default: derived from --rpc)
//!   --leader-key <path>    leader/quote-authority keypair (default keys/EEEE.json)
//!   --market <symbol>      quote only this market (repeatable); default: all
//!   --dry-run              poll feeds and print the intended quotes, then exit
//!   --drop <tier>          dry-run only: suppress pyth | coinbase | kraken |
//!                          coingecko | cmc | fx

use anyhow::{anyhow, Context, Result};
use dropset_fair_value::{FairValueConfig, FairValueEngine, Reading, Regime};
use dropset_feeds::venues::{
    CmcSource, CoinGeckoSource, CoinbaseTicker, FrankfurterSource, KrakenSource, PythFeed,
    PythHermesSource,
};
use dropset_feeds::{forward_channel, run_until, HttpClient, RunConfig, Sink, Source};
use dropset_maker_bot::config::{
    BotConfig, FeedConfig, MarketConfig, DEFAULT_LEADER_KEY, MARKETS, QUOTE_KEYPAIR_FILE,
    USDC_COINGECKO_ID, USDC_KRAKEN_PAIR,
};
use dropset_maker_bot::context::Context as BotContext;
use dropset_maker_bot::model::fair_mid::build_legs;
use dropset_maker_bot::quote_state::QuoteStateStore;
use dropset_maker_bot::tasks::FeedReceivers;
use dropset_maker_bot::{chain, fills, tasks};
use dropset_util::rpc::ws_url_from_rpc;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::collections::HashMap;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;

/// The live-sink channel bound between a feed source and the supervisor. Price
/// maps arrive once per poll interval and the tick drains every 5 s, so a small
/// buffer never fills; the sink drops to the latest if one ever did.
const FEED_CHANNEL_CAP: usize = 64;

/// How long a price source waits to retry after a failed poll — deliberately
/// far shorter than the steady-state poll interval (300 s for FX). The keyless
/// CoinGecko / Frankfurter tiers rate-limit, so a transient failure must
/// recover within a tick or two rather than dark the anchor for a whole
/// interval; the static peg covers the gap meanwhile.
const FEED_ERROR_BACKOFF: Duration = Duration::from_secs(20);

/// Lamports per SOL.
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
/// Below this leader balance, airdrop on startup (localnet).
const MIN_LEADER_LAMPORTS: u64 = LAMPORTS_PER_SOL / 2;
/// Airdrop size when topping up the leader.
const AIRDROP_LAMPORTS: u64 = 2 * LAMPORTS_PER_SOL;

struct Args {
    leader_key: String,
    dry_run: bool,
    /// Tiers to suppress in a dry run (to exercise the cascade).
    drop: Vec<String>,
    /// Symbols to restrict this instance to (empty = every market). The TUI
    /// runs one instance per market by passing a single `--market`, so an
    /// operator can start / stop each market's bot independently.
    markets: Vec<String>,
}

impl Args {
    /// The roster this instance quotes: every [`MarketConfig`] whose symbol was
    /// named with `--market` (case-insensitive), or all of them when none was.
    fn selected(&self) -> Vec<&'static MarketConfig> {
        MARKETS
            .iter()
            .filter(|m| {
                self.markets.is_empty()
                    || self
                        .markets
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(m.symbol))
            })
            .collect()
    }
}

fn main() -> Result<()> {
    // Surface the feeds runner's tracing diagnostics (a price / fill source
    // failing and backing off) on stderr; the framework has no other failure
    // signal. Default to `info`; `RUST_LOG` overrides.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut cfg = BotConfig::default();
    let args = parse_args(&mut cfg);
    if args.dry_run {
        dry_run(&cfg, &args)
    } else {
        run_live(&cfg, &args)
    }
}

/// Parse flags, mutating `cfg` and returning the run options.
fn parse_args(cfg: &mut BotConfig) -> Args {
    let mut leader_key = DEFAULT_LEADER_KEY.to_string();
    let mut dry_run = false;
    let mut drop = Vec::new();
    let mut markets = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rpc" => {
                if let Some(url) = it.next() {
                    cfg.rpc_url = url;
                }
            }
            "--ws" => {
                if let Some(url) = it.next() {
                    cfg.ws_url = Some(url);
                }
            }
            "--leader-key" => {
                if let Some(path) = it.next() {
                    leader_key = path;
                }
            }
            "--market" => {
                if let Some(symbol) = it.next() {
                    markets.push(symbol);
                }
            }
            "--drop" => {
                if let Some(tier) = it.next() {
                    drop.push(tier);
                }
            }
            "--dry-run" => dry_run = true,
            _ => {}
        }
    }
    Args {
        leader_key,
        dry_run,
        drop,
        markets,
    }
}

/// Discover the markets, fund the leader, and run the supervisor loop.
fn run_live(cfg: &BotConfig, args: &Args) -> Result<()> {
    let client = chain::rpc(&cfg.rpc_url);
    // Guard before funding or signing anything: the airdrop needs the localnet
    // faucet and the leader key holds no authority on a public cluster, so an
    // off-localnet --rpc is always a misconfiguration — fail fast rather than
    // emit doomed sends.
    chain::assert_localnet(&client)?;
    let leader = solana_keypair::read_keypair_file(&args.leader_key)
        .map_err(|e| anyhow!("read leader key {}: {e}", args.leader_key))?;

    // The leader pays for its own quoting txns; top it up on localnet.
    let balance = client
        .get_balance(&leader.pubkey())
        .context("leader balance")?;
    if balance < MIN_LEADER_LAMPORTS {
        println!(
            "funding leader {} ({} SOL)…",
            leader.pubkey(),
            AIRDROP_LAMPORTS / LAMPORTS_PER_SOL
        );
        chain::airdrop(&client, &leader.pubkey(), AIRDROP_LAMPORTS)?;
    }

    // Discover every on-chain market once, then match the roster against it by
    // base mint (quote is always USDC). The roster is narrowed to any
    // `--market` symbols so one instance can quote a single market.
    let discovered = chain::discover_markets(&client)?;
    let quote_mint = mint_pubkey(QUOTE_KEYPAIR_FILE)?;
    let roster = args.selected();
    // The persisted last-live-stamp records, one file per market — the evidence
    // the supervisor's startup pass ages a resting book against.
    let quote_state = QuoteStateStore::new(&cfg.invalidate.state_dir);
    let mut contexts = Vec::new();
    for &market in &roster {
        let base_mint = match mint_pubkey(market.base_keypair_file) {
            Ok(pk) => pk,
            Err(e) => {
                eprintln!("[{}] skipped — {e}", market.symbol);
                continue;
            }
        };
        let Some(addrs) = discovered
            .iter()
            .find(|m| m.base_mint == base_mint && m.quote_mint == quote_mint)
        else {
            eprintln!(
                "[{}] no on-chain market for {base_mint}/USDC — bootstrap it first",
                market.symbol
            );
            continue;
        };
        println!(
            "[{}] market {} ({})",
            market.symbol, addrs.market, base_mint
        );
        contexts.push(BotContext::new(
            chain::rpc(&cfg.rpc_url),
            leader.insecure_clone(),
            cfg.vault_idx,
            addrs.clone(),
            *market,
            cfg.fair_value,
            quote_state.for_market(addrs.market, market.symbol),
        ));
    }
    if contexts.is_empty() {
        return Err(anyhow!(
            "no demo markets found on-chain — is the localnet bootstrapped?"
        ));
    }

    // A small background runtime drives the async feed sources; the tick loop
    // below stays synchronous and reads the broadcast tails. `enable_all` backs
    // the reqwest reactor the HTTP price sources need.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("build feeds runtime")?;

    // The price tiers, batched across the quoted roster: each venue is polled
    // once for every market it can price.
    let markets: Vec<&MarketConfig> = contexts.iter().map(|c| &c.cfg).collect();
    let feeds = spawn_price_feeds(&rt, &cfg.feeds, &FeedRoster::for_markets(&markets))?;

    // One fill subscription covers every market the leader quotes; its
    // `logsSubscribe` socket bridges through the feeds stream seam onto a live
    // sink, and the supervisor routes each fill to its market by `event.market`.
    let ws_url = cfg
        .ws_url
        .clone()
        .unwrap_or_else(|| ws_url_from_rpc(&cfg.rpc_url));
    let fills = fills::spawn(ws_url, cfg.rpc_url.clone(), leader.pubkey())
        .map(|source| spawn_feed(&rt, source, RunConfig::default()));

    // The runtime must outlive the supervisor loop that reads its channels; it
    // does — `run_supervisor` runs until the process is killed, and `rt` is held
    // in this frame the whole time.
    tasks::run_supervisor(feeds, cfg.clone(), contexts, fills)
}

/// Spawn a feeds `source` on `rt`, forwarding its records onto an in-process
/// live sink, and return the receiver the supervisor drains. The runner is
/// given a never-resolving shutdown (`pending`) so it lives with the process: a
/// demo feed has no cursor to flush on exit, and installing a ctrl-c handler
/// here (as `feeds::run` does) would swallow the signal that stops the
/// synchronous tick loop.
fn spawn_feed<S>(rt: &Runtime, source: S, cfg: RunConfig) -> broadcast::Receiver<S::Record>
where
    S: Source + Send + 'static,
    S::Record: Clone + Send + Sync + 'static,
{
    let (sink, rx) = forward_channel(FEED_CHANNEL_CAP);
    let sinks: Vec<Box<dyn Sink<S::Record>>> = vec![Box::new(sink)];
    rt.spawn(async move {
        if let Err(e) = run_until(source, sinks, cfg, std::future::pending::<()>()).await {
            eprintln!("[feed] runner exited: {e}");
        }
    });
    rx
}

/// Every venue's symbol set for the markets this run quotes, derived once so
/// the live path and `--dry-run` batch identically instead of each rebuilding
/// the rosters from `MarketConfig` and drifting apart.
struct FeedRoster {
    /// Pyth FX feeds, deduped by currency — several markets can track one fiat.
    pyth: Vec<PythFeed>,
    /// Kraken pairs: each listed token, plus the shared USDC/USD peg leg.
    kraken: Vec<String>,
    /// Coinbase product ids, one source each (the ticker endpoint is per
    /// product, so this is not a batch).
    coinbase: Vec<String>,
    /// CoinGecko ids, including the USDC common-mode fallback.
    coingecko: Vec<String>,
    /// CoinMarketCap numeric ids — only the markets that name one.
    coinmarketcap: Vec<u32>,
    /// ISO codes for the Frankfurter batch, deduped **across** markets: two
    /// tokens tracking one fiat share a currency, so this is not one per
    /// market.
    currencies: Vec<String>,
}

impl FeedRoster {
    fn for_markets(markets: &[&MarketConfig]) -> Self {
        let mut currencies: Vec<String> = markets.iter().map(|m| m.currency.to_string()).collect();
        currencies.sort_unstable();
        currencies.dedup();

        // One Pyth feed per *currency*, not per market: two tokens tracking the
        // same fiat share one cross, and asking Hermes for it twice would only
        // pay the request twice.
        let mut pyth: Vec<PythFeed> = Vec::new();
        for m in markets {
            if pyth.iter().any(|f| f.key == m.currency) {
                continue;
            }
            pyth.push(if m.pyth_invert {
                PythFeed::inverted(m.currency, m.pyth_feed_id)
            } else {
                PythFeed::direct(m.currency, m.pyth_feed_id)
            });
        }

        // The USDC peg leg rides the batched Kraken call the token pairs use.
        let mut kraken: Vec<String> = markets
            .iter()
            .filter_map(|m| m.kraken_pair.map(str::to_string))
            .collect();
        kraken.push(USDC_KRAKEN_PAIR.to_string());
        kraken.sort_unstable();
        kraken.dedup();

        let mut coinbase: Vec<String> = markets
            .iter()
            .filter_map(|m| m.coinbase_product.map(str::to_string))
            .collect();
        coinbase.sort_unstable();
        coinbase.dedup();

        // The USDC/USD common-mode fallback rides the batched CoinGecko call.
        let mut coingecko: Vec<String> = markets
            .iter()
            .filter_map(|m| m.coingecko_id.map(str::to_string))
            .collect();
        coingecko.push(USDC_COINGECKO_ID.to_string());
        coingecko.sort_unstable();
        coingecko.dedup();

        let mut coinmarketcap: Vec<u32> =
            markets.iter().filter_map(|m| m.coinmarketcap_id).collect();
        coinmarketcap.sort_unstable();
        coinmarketcap.dedup();

        Self {
            pyth,
            kraken,
            coinbase,
            coingecko,
            coinmarketcap,
            currencies,
        }
    }
}

/// Spawn every price source on `rt` and bundle their live-sink receivers.
/// Each source owns its steady-state poll cadence (its `RunConfig::poll_interval`)
/// but retries on the short [`FEED_ERROR_BACKOFF`] so a transient rate-limit
/// doesn't dark the anchor for a whole interval. CoinMarketCap is wired whenever
/// the roster names any CMC id — it needs no credential, being on the keyless
/// public route; the Coinbase tier is empty unless a quoted market is actually
/// listed there.
fn spawn_price_feeds(rt: &Runtime, cfg: &FeedConfig, roster: &FeedRoster) -> Result<FeedReceivers> {
    let pyth = spawn_feed(
        rt,
        PythHermesSource::new(&cfg.pyth_base_url, roster.pyth.clone())?,
        RunConfig {
            poll_interval: cfg.pyth_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
    );
    let kraken = spawn_feed(
        rt,
        KrakenSource::new(&cfg.kraken_base_url, roster.kraken.clone())?,
        RunConfig {
            poll_interval: cfg.kraken_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
    );
    // Coinbase's ticker endpoint is per product, so a roster of N listed tokens
    // is N sources rather than one batched poll. They share **one cloned
    // client** so they also share one rate gate: Coinbase throttles by IP, and
    // N independently-constructed clients would each pace against the venue
    // alone while all N spent the same bucket.
    let coinbase_http = HttpClient::new(&cfg.coinbase_base_url)?;
    let coinbase = roster
        .coinbase
        .iter()
        .map(|product| {
            spawn_feed(
                rt,
                CoinbaseTicker::from_client(coinbase_http.clone(), product.clone()),
                RunConfig {
                    poll_interval: cfg.coinbase_poll,
                    error_backoff: FEED_ERROR_BACKOFF,
                },
            )
        })
        .collect::<Vec<_>>();
    let coingecko = spawn_feed(
        rt,
        CoinGeckoSource::new(&cfg.coingecko_base_url, roster.coingecko.clone())?,
        RunConfig {
            poll_interval: cfg.coingecko_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
    );
    // Wired whenever any selected market names a CMC id — no credential to
    // gate on, since this adapter is on the keyless public route.
    let coinmarketcap = if roster.coinmarketcap.is_empty() {
        None
    } else {
        Some(spawn_feed(
            rt,
            CmcSource::new(&cfg.coinmarketcap_base_url, roster.coinmarketcap.clone())?,
            RunConfig {
                poll_interval: cfg.coinmarketcap_poll,
                error_backoff: FEED_ERROR_BACKOFF,
            },
        ))
    };
    let frankfurter = spawn_feed(
        rt,
        FrankfurterSource::new(&cfg.frankfurter_base_url, roster.currencies.clone())?,
        RunConfig {
            poll_interval: cfg.fx_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
    );
    Ok(FeedReceivers {
        pyth,
        kraken,
        coinbase,
        coingecko,
        coinmarketcap,
        frankfurter,
    })
}

/// Load a checked-in mint keypair and return its public key.
fn mint_pubkey(keypair_file: &str) -> Result<Pubkey> {
    solana_keypair::read_keypair_file(keypair_file)
        .map(|kp| kp.pubkey())
        .map_err(|e| anyhow!("read mint key {keypair_file}: {e}"))
}

/// Poll the tiered feeds once and print the reference each market would stamp.
/// No validator and no writes — a credentials/cascade check. `--drop` suppresses
/// a tier so the cascade to the next one is observable.
fn dry_run(cfg: &BotConfig, args: &Args) -> Result<()> {
    // One-shot polls on a current-thread runtime — no supervisor, no live sink.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build dry-run runtime")?;
    let drop = |tier: &str| args.drop.iter().any(|d| d == tier);

    // Named to match `run_live`, where `markets` is the selection and `roster`
    // is the derived per-venue symbol sets — the two paths should read alike.
    let markets = args.selected();
    let roster = FeedRoster::for_markets(&markets);

    // Each tier is polled once, and a failed poll is indistinguishable from a
    // suppressed one here on purpose: both leave the tier empty and let the
    // cascade below show what still prices.
    let pyth = if drop("pyth") {
        Default::default()
    } else {
        rt.block_on(PythHermesSource::new(&cfg.feeds.pyth_base_url, roster.pyth)?.poll())
            .unwrap_or_default()
    };
    let kraken = if drop("kraken") {
        Default::default()
    } else {
        rt.block_on(KrakenSource::new(&cfg.feeds.kraken_base_url, roster.kraken)?.poll())
            .unwrap_or_default()
    };
    let mut coinbase: HashMap<String, f64> = HashMap::new();
    if !drop("coinbase") {
        // One client cloned across the products, as in `spawn_price_feeds`, so
        // the per-product polls share a single rate gate against the venue.
        let http = HttpClient::new(&cfg.feeds.coinbase_base_url)?;
        for product in &roster.coinbase {
            let ticker = CoinbaseTicker::from_client(http.clone(), product.clone());
            if let Ok(Some(price)) = rt.block_on(ticker.poll()) {
                coinbase.insert(product.clone(), price);
            }
        }
    }
    let cg = if drop("coingecko") {
        Default::default()
    } else {
        rt.block_on(CoinGeckoSource::new(&cfg.feeds.coingecko_base_url, roster.coingecko)?.poll())
            .unwrap_or_default()
    };
    let cmc = if drop("cmc") || roster.coinmarketcap.is_empty() {
        Default::default()
    } else {
        rt.block_on(CmcSource::new(&cfg.feeds.coinmarketcap_base_url, roster.coinmarketcap)?.poll())
            .unwrap_or_default()
    };
    let fx = if drop("fx") {
        Default::default()
    } else {
        rt.block_on(
            FrankfurterSource::new(&cfg.feeds.frankfurter_base_url, roster.currencies)?.poll(),
        )
        .unwrap_or_default()
    };

    println!(
        "Tiers live: pyth {} feeds, coinbase {} products, kraken {} pairs, \
         coingecko {} ids, coinmarketcap {} ids, fx {} currencies",
        pyth.len(),
        coinbase.len(),
        kraken.len(),
        cg.len(),
        cmc.len(),
        fx.len()
    );
    if !args.drop.is_empty() {
        println!("Suppressed tiers: {}", args.drop.join(", "));
    }
    // Column widths fit the longest value each can take: `Unverified` for
    // health, and a pinned basis rendered as `1.0000 pinned`.
    println!(
        "\n  market      mid (USDC)    anchor         health       \
         basis           fx source"
    );

    let now = Duration::from_secs(0);
    let q = |v: Option<f64>| v.map(|v| Reading::new(v, now));
    // USDC/USD common-mode leg, shared by every market: Kraken's market print,
    // falling back to the CoinGecko index.
    let usdc_q =
        q(kraken.get(USDC_KRAKEN_PAIR).copied()).or_else(|| q(cg.get(USDC_COINGECKO_ID).copied()));
    for &m in &markets {
        // FX anchor: Pyth (with its confidence half-width) over Frankfurter.
        // A dry run has no wall-clock history, so every reading is age zero and
        // `pyth_reading`'s publish-time ageing has nothing to bite on — the
        // point here is which tier answered, not staleness.
        let (fx_q, fx_src) = match pyth.get(m.currency) {
            Some(p) => {
                let reading = match p.confidence {
                    Some(conf) => Reading::with_confidence(p.value, now, conf),
                    None => Reading::new(p.value, now),
                };
                (Some(reading), "pyth")
            }
            None => match q(fx.get(m.currency).copied()) {
                Some(r) => (Some(r), "frankfurter"),
                None => (None, "—"),
            },
        };
        // Basis leg, in preference order: Coinbase token/USDC, Kraken
        // token/USD, then the reflexive CoinGecko / CMC index. Kraken's USD
        // quote is converted with the peg leg, exactly as `FeedHub::legs`
        // does — the two walks must agree or a dry run stops predicting the
        // live mid.
        let usdc_per_usd = usdc_q.map(|r| r.value).filter(|v| *v > 0.0);
        let basis_q = m
            .coinbase_product
            .and_then(|p| q(coinbase.get(p).copied()))
            .or_else(|| {
                m.kraken_pair.and_then(|p| {
                    q(kraken.get(p).copied()).map(|r| match usdc_per_usd {
                        Some(peg) => Reading::new(r.value / peg, now),
                        None => r,
                    })
                })
            })
            .or_else(|| q(m.coingecko_id.and_then(|id| cg.get(id)).copied()))
            .or_else(|| q(m.coinmarketcap_id.and_then(|id| cmc.get(&id)).copied()));
        // A fresh engine per row — no history, so no smoothing. The pinned
        // basis is per-market, so it is layered onto the shared calibration
        // here exactly as the live path does in `Context`.
        let legs = build_legs(fx_q, basis_q, usdc_q, m.static_usd);
        let mut engine = FairValueEngine::new(FairValueConfig {
            pinned_basis: m.pinned_basis,
            ..cfg.fair_value
        });
        let fair = engine.compose(legs, now, false);
        let anchor = format!("{:?}", fair.anchor);
        // Rendered to a String first: a derived `Debug` ignores width specifiers,
        // so `{:<11?}` would not pad and the column would drift on the longer
        // variants.
        let health = format!("{:?}", fair.health);
        let mid = fair.fair.map_or("—".to_string(), |v| format!("{v:.8}"));
        let basis = fair.basis.map_or("—".to_string(), |b| {
            let note = if fair.regime == Regime::FxPinned {
                // Named, not blank: a bare 1.0000 here would read as an
                // observed basis that happens to sit at parity.
                " pinned"
            } else if fair.basis_breach {
                " BREACH"
            } else {
                ""
            };
            format!("{b:.4}{note}")
        });
        println!(
            "  {:<10}  {:>12}  {:<13}  {:<11}  {:<14}  {}",
            m.symbol,
            mid,
            anchor,
            health,
            basis,
            if fair.uncertain {
                format!("{fx_src} (wide)")
            } else {
                fx_src.to_string()
            }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(markets: &[&str]) -> Args {
        Args {
            leader_key: DEFAULT_LEADER_KEY.to_string(),
            dry_run: false,
            drop: Vec::new(),
            markets: markets.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn no_market_flag_selects_the_whole_roster() {
        assert_eq!(args(&[]).selected().len(), MARKETS.len());
    }

    #[test]
    fn market_flag_narrows_to_the_named_markets_case_insensitively() {
        let selected = args(&["eurc", "MXNE"]).selected();
        let symbols: Vec<&str> = selected.iter().map(|m| m.symbol).collect();
        assert_eq!(symbols, ["EURC", "MXNe"]);
    }

    #[test]
    fn an_unknown_market_symbol_selects_nothing() {
        assert!(args(&["nope"]).selected().is_empty());
    }

    /// The shared USDC legs must survive a single-`--market` run. The TUI
    /// starts one bot per market, so this is the *normal* case, not an edge —
    /// and dropping either leg silently disables the portfolio-wide
    /// common-mode guard (§1 fm1) rather than failing loudly.
    #[test]
    fn the_shared_usdc_legs_ride_every_roster_however_narrow() {
        for selection in [vec![], vec!["EURC"], vec!["IDRX"]] {
            let markets = args(&selection).selected();
            let roster = FeedRoster::for_markets(&markets);
            assert!(
                roster.kraken.iter().any(|p| p == USDC_KRAKEN_PAIR),
                "{selection:?} lost the Kraken peg pair"
            );
            assert!(
                roster.coingecko.iter().any(|i| i == USDC_COINGECKO_ID),
                "{selection:?} lost the CoinGecko peg fallback"
            );
        }
    }

    /// One Pyth feed per *currency*, not per market — and every venue roster
    /// deduped, so no batch asks a venue for the same symbol twice.
    #[test]
    fn feed_roster_batches_each_symbol_exactly_once() {
        let markets = args(&[]).selected();
        let roster = FeedRoster::for_markets(&markets);

        let currencies: Vec<&str> = roster.pyth.iter().map(|f| f.key.as_str()).collect();
        let mut deduped = currencies.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(currencies.len(), deduped.len(), "duplicate Pyth currency");

        for (label, len, deduped_len) in [
            ("kraken", roster.kraken.len(), dedup_len(&roster.kraken)),
            (
                "coinbase",
                roster.coinbase.len(),
                dedup_len(&roster.coinbase),
            ),
            (
                "coingecko",
                roster.coingecko.len(),
                dedup_len(&roster.coingecko),
            ),
            (
                "currencies",
                roster.currencies.len(),
                dedup_len(&roster.currencies),
            ),
        ] {
            assert_eq!(len, deduped_len, "{label} roster has duplicates");
        }
        let mut cmc = roster.coinmarketcap.clone();
        cmc.dedup();
        assert_eq!(cmc.len(), roster.coinmarketcap.len(), "cmc has duplicates");
    }

    fn dedup_len(v: &[String]) -> usize {
        let mut c = v.to_vec();
        c.sort_unstable();
        c.dedup();
        c.len()
    }

    /// Only the markets a venue actually lists reach that venue's batch — the
    /// counterpart to the config-side test that no market claims a listing it
    /// does not have.
    #[test]
    fn a_venue_batch_carries_only_the_markets_it_lists() {
        let markets = args(&["IDRX", "ZARP"]).selected();
        let roster = FeedRoster::for_markets(&markets);
        // Neither is on Coinbase, so that tier spawns no source at all.
        assert!(roster.coinbase.is_empty());
        // Neither is on Kraken either — only the shared peg pair remains.
        assert_eq!(roster.kraken, vec![USDC_KRAKEN_PAIR.to_string()]);
        // But both still get an FX anchor and an index fallback.
        assert_eq!(roster.pyth.len(), 2);
        assert_eq!(
            roster.currencies,
            vec!["IDR".to_string(), "ZAR".to_string()]
        );
    }

    /// The inversion flag has to travel with the feed id, not be re-derived —
    /// five of the seven roster currencies are published as `USD/<ccy>`.
    #[test]
    fn pyth_feeds_carry_each_markets_own_direction() {
        let markets = args(&["EURC", "ZARP"]).selected();
        let roster = FeedRoster::for_markets(&markets);
        let eur = roster.pyth.iter().find(|f| f.key == "EUR").unwrap();
        let zar = roster.pyth.iter().find(|f| f.key == "ZAR").unwrap();
        assert!(!eur.invert, "EUR/USD is published direct");
        assert!(zar.invert, "ZAR is published as USD/ZAR");
    }
}
