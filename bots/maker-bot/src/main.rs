//! `dropset-maker-bot` entrypoint.
//!
//! Default mode supervises every demo market live against a localnet validator:
//! discover the markets, fund the leader, and drive the tick loop, one batched
//! feed poll shared across them. `--dry-run` instead polls the tiered feeds
//! once and prints the reference each market *would* stamp — the wiring check
//! that every venue is reachable and decoding, with no validator and no writes.
//! Pass `--drop <tier>` (repeatable: `pyth`, `coinbase`, `kraken`, `coingecko`,
//! `cmc`, `fx`, `erapi`) in a dry run to suppress a tier and watch the cascade
//! fall through to the next one — dropping `pyth` is how you check the daily FX
//! references still carry the anchor, and dropping `fx` leaves er-api as the
//! only one, which is the shape the NGN markets run in permanently.
//!
//! Flags:
//!   --rpc <url>            RPC endpoint (default http://127.0.0.1:8899)
//!   --ws <url>             PubSub websocket (default: derived from --rpc)
//!   --leader-key <path>    leader/quote-authority keypair (default keys/EEEE.json)
//!   --market <symbol>      quote only this market (repeatable); default: all
//!   --dry-run              poll feeds and print the intended quotes, then exit
//!   --drop <tier>          dry-run only: suppress pyth | coinbase | kraken |
//!                          coingecko | cmc | fx | erapi

use anyhow::{anyhow, Context, Result};
use dropset_fair_value::{
    Candidates, ClockCtx, ConsensusState, FairValueEngine, LegReport, Reading, Regime,
};
use dropset_feeds::venues::{
    CmcSource, CoinGeckoSource, CoinbaseTicker, ErApiSource, FrankfurterSnapshotSource,
    FrankfurterSource, KrakenSource, PythFeed, PythHermesSource,
};
use dropset_feeds::{
    connect_lazy, forward_channel, parked_source, redact_to_origin, run_until,
    run_until_with_metrics, HttpClient, RunConfig, Sink, Source, MAX_ERROR_CHARS, PARKED_SOURCES,
};
use dropset_maker_bot::config::{
    BotConfig, FeedConfig, MarketConfig, DEFAULT_LEADER_KEY, MARKETS, QUOTE_KEYPAIR_FILE,
    USDC_COINGECKO_ID, USDC_KRAKEN_PAIR,
};
use dropset_maker_bot::context::Context as BotContext;
use dropset_maker_bot::fx_store::{self, FxStoreSource};
use dropset_maker_bot::model::fair_mid::build_legs;
use dropset_maker_bot::quote_state::QuoteStateStore;
use dropset_maker_bot::tasks::{
    FeedReceivers, SOURCE_CMC, SOURCE_COINBASE, SOURCE_COINGECKO, SOURCE_ERAPI, SOURCE_FRANKFURTER,
    SOURCE_KRAKEN, SOURCE_PYTH,
};
use dropset_maker_bot::telemetry::{self, Telemetry};
use dropset_maker_bot::{chain, fills, tasks};
use dropset_util::rpc::ws_url_from_rpc;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::collections::HashMap;
use std::sync::Arc;
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
    // One signing handle for the whole process, shared by every market's
    // context, so roster size doesn't multiply the number of long-lived
    // copies of the key material.
    let leader = Arc::new(
        solana_keypair::read_keypair_file(&args.leader_key)
            .map_err(|e| anyhow!("read leader key {}: {e}", args.leader_key))?,
    );

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

    // A small background runtime drives the async feed sources and the
    // telemetry drain; the tick loop below stays synchronous and reads the
    // broadcast tails. `enable_all` backs the reqwest reactor the HTTP price
    // sources need, and the Postgres pool the telemetry sink writes through.
    //
    // Built here — before the per-market contexts rather than beside the feed
    // spawns further down — because each context carries a telemetry handle,
    // and that handle can only exist once there is a runtime to drain it.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("build feeds runtime")?;

    // The operational read path. A disabled handle when no telemetry database
    // is configured or reachable, which is the normal localnet default — the
    // bot quotes either way.
    let telemetry = telemetry::spawn(&rt);

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
            Arc::clone(&leader),
            cfg.vault_idx,
            addrs.clone(),
            *market,
            cfg.fair_value,
            quote_state.for_market(addrs.market, market.symbol),
            telemetry.clone(),
        ));
    }
    if contexts.is_empty() {
        return Err(anyhow!(
            "no demo markets found on-chain — is the localnet bootstrapped?"
        ));
    }

    // The price tiers, batched across the quoted roster: each venue is polled
    // once for every market it can price.
    let markets: Vec<&MarketConfig> = contexts.iter().map(|c| &c.cfg).collect();
    let feeds = spawn_price_feeds(
        &rt,
        &cfg.feeds,
        &FeedRoster::for_markets(&markets),
        &telemetry,
    )?;

    // One fill subscription covers every market the leader quotes; its
    // `logsSubscribe` socket bridges through the feeds stream seam onto a live
    // sink, and the supervisor routes each fill to its market by `event.market`.
    let ws_url = cfg
        .ws_url
        .clone()
        .unwrap_or_else(|| ws_url_from_rpc(&cfg.rpc_url));
    // `HealthRow::Skip`: a push source's silence is its healthy state, so a
    // staleness row for it would page falsely on any quiet market. See
    // `HealthRow`.
    //
    // It is not unmonitored, though — it reports the one thing a push source
    // *can* report honestly. The liveness reporter goes to the subscription
    // thread rather than to the runner, because the transport's state is only
    // observable to the code that owns the socket, and lands in `push_health`
    // instead. Named here rather than derived, since unlike the health seam
    // there is no source to auto-register from.
    let fills = fills::spawn(
        ws_url,
        cfg.rpc_url.clone(),
        leader.pubkey(),
        telemetry.liveness_reporter(fills::FILLS_FEED),
    )
    .map(|source| {
        spawn_feed(
            &rt,
            source,
            RunConfig::default(),
            &telemetry,
            HealthRow::Skip,
        )
    });

    // The runtime must outlive the supervisor loop that reads its channels; it
    // does — `run_supervisor` runs until the process is killed, and `rt` is held
    // in this frame the whole time.
    tasks::run_supervisor(feeds, cfg.clone(), contexts, fills)
}

/// Whether a spawned source contributes a `feed_health` row.
///
/// Every **polled** source should: that table plus its staleness rule is how
/// a silent venue is noticed, and auto-registration is the point of driving
/// them all through one seam.
///
/// A **push** source must not, and the reason is that staleness is not
/// defined for it. `ChannelSource::next` blocks until a record arrives, so
/// the runner reports a batch only when the transport delivers one — for the
/// fill subscription that means the health row's last-success timestamp
/// tracks *the last trade*, not the last time the socket was known good. On a
/// market that simply has no fills for half an hour, which is the ordinary
/// localnet state and common on a thin real market, that row ages out and the
/// generic stale-feed rule pages saying a **price** feed died, pointing the
/// operator at the wrong panel entirely. No threshold fixes it: silence is
/// this source's healthy state, so nothing distinguishes a dead socket from a
/// quiet market. The honest position is that this table does not cover push
/// sources.
///
/// `Skip` therefore means "not through *this* seam", not "unmonitored". A push
/// source reports its transport state instead — `feeds`'
/// `LivenessReporter`, wired at the producer rather than the runner, writing
/// `push_health` — which is the signal a dead socket actually has. The two are
/// alerted on differently and must not be confused: this one pages on a stale
/// `last_ok_at`, that one on a link that is not up.
enum HealthRow {
    /// Report liveness — every polled price source.
    Report,
    /// Report nothing *here*, for a push source whose silence is normal. Its
    /// transport state goes to `push_health` via a liveness reporter handed to
    /// the producer.
    Skip,
}

/// Pyth's **bare venue token**, the key `PARKED_SOURCES` is indexed by.
///
/// Deliberately not [`SOURCE_PYTH`], which is the fusion attribution tag
/// (`pyth-hermes`) and would silently match no park record — the two
/// vocabularies are separate on purpose and this is the kind of mix-up that
/// fails by returning `None`, i.e. by reading as "not parked". Spelled here
/// rather than imported from the adapter for the same reason `SOURCE_ERAPI` is:
/// agreeing with a collector's `spot_ticks.source` today is convenient, not an
/// invariant either side guarantees.
const VENUE_PYTH: &str = "pyth";

/// A receiver for a tier that will never deliver: an empty channel whose sender
/// is dropped immediately.
///
/// `drain_into` treats `Closed` exactly as it treats `Empty` — it stops and
/// leaves the cache alone — so a parked tier's cache stays empty and its leg
/// resolves to `None`, which is the same state a spawned-but-failing tier
/// reaches. That equivalence is the point: parking a source changes what gets
/// logged, not how the cascade prices. A unit test in `tasks.rs` pins the
/// `Closed`-drains-like-`Empty` half that this function relies on.
fn parked_receiver<T: Clone>() -> broadcast::Receiver<T> {
    let (_, rx) = broadcast::channel(1);
    rx
}

/// Spawn a feeds `source` on `rt`, forwarding its records onto an in-process
/// live sink, and return the receiver the supervisor drains. The runner is
/// given a never-resolving shutdown (`pending`) so it lives with the process: a
/// demo feed has no cursor to flush on exit, and installing a ctrl-c handler
/// here (as `feeds::run` does) would swallow the signal that stops the
/// synchronous tick loop.
///
/// A polled source spawned here is driven through `run_until_with_metrics`
/// with a health recorder attached, which is what makes the `feed_health`
/// table complete by construction: the recorder keys on the source's own name,
/// so a venue adapter added later reports without this function learning
/// anything about it. That completeness now has one exception: a caller that
/// gates on `PARKED_SOURCES` — today only the Pyth tier — skips this function
/// entirely for a parked venue, which then contributes no health row at all
/// (see `parked_receiver`). Note the exception is a property of the *call
/// site*, not of the set: an entry nobody gated on still reaches this function
/// and still reports.
///
/// When telemetry is disabled the plain `run_until` is
/// used, so a run with no database costs nothing rather than reporting into a
/// dead channel.
///
/// `health` is what a **push** source opts out with — see [`HealthRow`].
fn spawn_feed<S>(
    rt: &Runtime,
    source: S,
    cfg: RunConfig,
    telemetry: &Telemetry,
    health: HealthRow,
) -> broadcast::Receiver<S::Record>
where
    S: Source + Send + 'static,
    S::Record: Clone + Send + Sync + 'static,
{
    let (sink, rx) = forward_channel(FEED_CHANNEL_CAP);
    let sinks: Vec<Box<dyn Sink<S::Record>>> = vec![Box::new(sink)];
    let reporter = match health {
        HealthRow::Report => telemetry.health_reporter(),
        HealthRow::Skip => None,
    };
    match reporter {
        Some(metrics) => {
            rt.spawn(async move {
                if let Err(e) = run_until_with_metrics(
                    source,
                    sinks,
                    cfg,
                    std::future::pending::<()>(),
                    metrics,
                )
                .await
                {
                    eprintln!("[feed] runner exited: {e}");
                }
            });
        }
        None => {
            rt.spawn(async move {
                if let Err(e) = run_until(source, sinks, cfg, std::future::pending::<()>()).await {
                    eprintln!("[feed] runner exited: {e}");
                }
            });
        }
    }
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
fn spawn_price_feeds(
    rt: &Runtime,
    cfg: &FeedConfig,
    roster: &FeedRoster,
    telemetry: &Telemetry,
) -> Result<FeedReceivers> {
    // Keyless on purpose: the bot resolves no secrets, and wiring a credential
    // into it is deliberately out of scope here. Against a self-hosted Hermes
    // it works unchanged — but `pyth_base_url` is a compile-time default with
    // no override, so reaching one is a code edit that also clears the park.
    let pyth = match parked_source(VENUE_PYTH) {
        // Parked: don't spawn a poller that cannot succeed. Keyless against
        // upstream Hermes every poll 401s, and the runner logs one `warn!` per
        // poll for the life of the process — a per-tick failure report for a
        // tier that is off by decision, which is noise rather than signal.
        Some(park) => {
            // `eprintln!` with a `[feed]` tag, matching this file's own
            // diagnostics: the `tracing` macros here belong to the feeds crate,
            // whose output the subscriber in `main` renders, and the bot is not
            // a direct dependent of `tracing`.
            eprintln!(
                "[feed] {} parked by decision since {} — {}",
                park.venue, park.since, park.reason
            );
            parked_receiver()
        }
        None => spawn_feed(
            rt,
            PythHermesSource::new(&cfg.pyth_base_url, None, roster.pyth.clone())?,
            RunConfig {
                poll_interval: cfg.pyth_poll,
                error_backoff: FEED_ERROR_BACKOFF,
            },
            telemetry,
            HealthRow::Report,
        ),
    };
    let kraken = spawn_feed(
        rt,
        KrakenSource::new(&cfg.kraken_base_url, roster.kraken.clone())?,
        RunConfig {
            poll_interval: cfg.kraken_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
        telemetry,
        HealthRow::Report,
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
                telemetry,
                HealthRow::Report,
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
        telemetry,
        HealthRow::Report,
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
            telemetry,
            HealthRow::Report,
        ))
    };
    let frankfurter = spawn_feed(
        rt,
        // The snapshot variant, so the ECB reference date reaches the maker and
        // the fix can be aged from publication rather than receipt. The plain
        // `FrankfurterSource` stays parse-free and is still what the one-shot
        // dry-run below uses, which wants rates and no stamp.
        FrankfurterSnapshotSource::new(&cfg.frankfurter_base_url, roster.currencies.clone())?,
        RunConfig {
            poll_interval: cfg.fx_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
        telemetry,
        HealthRow::Report,
    );
    // The second daily reference, on the same roster and the same cadence as
    // Frankfurter — one batched request prices every currency, so widening the
    // roster costs this tier nothing.
    let erapi = spawn_feed(
        rt,
        ErApiSource::new(&cfg.erapi_base_url, roster.currencies.clone())?,
        RunConfig {
            poll_interval: cfg.fx_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
        telemetry,
        HealthRow::Report,
    );
    // The intraday FX anchor, read from the market-data store rather than
    // polled from the keyed venues (see `fx_store`). Unlike telemetry, which
    // degrades to silence when the database is absent, this one is on the
    // price path — so a missing URL is a startup error rather than a bot that
    // boots and then halts a few minutes later for reasons the operator has to
    // go and diagnose. Fail-closed, but fail *legibly*.
    let url = std::env::var(telemetry::DATABASE_URL_ENV).map_err(|_| {
        anyhow!(
            "{} is required: the intraday FX anchor is read from the \
             market-data store, and the maker will not quote without it",
            telemetry::DATABASE_URL_ENV
        )
    })?;
    // Lazy, like telemetry's: connecting here would make the bot's startup
    // wait on the database, and a connection that drops later has to be
    // survivable anyway — the source retries on its own backoff and the
    // silence bound is what turns a persistent failure into a halt.
    //
    // Built inside the runtime's context because a lazy pool still spawns its
    // idle reaper immediately, and `tokio::spawn` panics without a handle in
    // scope. This function is handed the runtime rather than running on it, so
    // that handle has to be entered explicitly.
    let pool = {
        let _guard = rt.enter();
        // Redacted to origin, matching the telemetry path's treatment of the
        // identical error. The two calls take the same input and only one of
        // them used to redact — and this is the call where a malformed URL is
        // guaranteed to produce an error, so it is the one whose cause chain
        // would carry the operator's password into stderr.
        connect_lazy(&url).map_err(|e| {
            let cause = redact_to_origin(&format!("{e:#}"), MAX_ERROR_CHARS);
            anyhow!(
                "{} is not a usable connection string ({cause})",
                telemetry::DATABASE_URL_ENV
            )
        })?
    };
    let fx_store = spawn_feed(
        rt,
        FxStoreSource::new(
            "store:fx",
            pool,
            roster
                .currencies
                .iter()
                .filter_map(|c| fx_store::fx_product_id(c))
                .collect(),
        ),
        RunConfig {
            poll_interval: cfg.fx_store_poll,
            error_backoff: FEED_ERROR_BACKOFF,
        },
        telemetry,
        HealthRow::Report,
    );
    Ok(FeedReceivers {
        pyth,
        kraken,
        coinbase,
        coingecko,
        coinmarketcap,
        frankfurter,
        erapi,
        fx_store,
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
        // Keyless for the same reason as the live path above.
        rt.block_on(PythHermesSource::new(&cfg.feeds.pyth_base_url, None, roster.pyth)?.poll())
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
    // Only the rates are kept: the dry run composes one tick from a single poll,
    // so there is no receipt age to compare a snapshot instant against and no
    // stall to detect across polls.
    let erapi = if drop("erapi") {
        Default::default()
    } else {
        rt.block_on(ErApiSource::new(&cfg.feeds.erapi_base_url, roster.currencies.clone())?.poll())
            .map(|snap| snap.rates)
            .unwrap_or_default()
    };
    // The store tier. Unlike the live path this tolerates an absent or
    // unreachable database: a dry run is a wiring check, and reporting "no
    // rows" is the diagnosis rather than a reason to refuse to run. The
    // fail-closed rule governs quoting, and a dry run does not quote.
    let fx_store_rows: Vec<_> = if drop("fx-store") {
        Vec::new()
    } else {
        // Two arms, not one `.ok()` chain. Collapsing them made the dry run
        // report "the variable is unset" when the variable was in fact set
        // and unparseable — telling the operator the easier of the two
        // failures while the harder one was the real state, in the one mode
        // whose entire job is to diagnose wiring.
        //
        // The enter guard is why this is not a plain `and_then`: a lazy pool
        // spawns its reaper on construction, so it has to be built inside the
        // runtime's context (see `spawn_price_feeds`).
        let pool = {
            let _guard = rt.enter();
            match std::env::var(telemetry::DATABASE_URL_ENV) {
                Ok(url) => match connect_lazy(&url) {
                    Ok(pool) => Some(pool),
                    Err(e) => {
                        let cause = redact_to_origin(&format!("{e:#}"), MAX_ERROR_CHARS);
                        eprintln!(
                            "[dry-run] {} is SET but not a usable connection string \
                             ({cause}) — the intraday FX tier is dark",
                            telemetry::DATABASE_URL_ENV
                        );
                        None
                    }
                },
                Err(_) => {
                    eprintln!(
                        "[dry-run] {} is unset — the intraday FX tier is dark, and \
                         the live bot would refuse to start",
                        telemetry::DATABASE_URL_ENV
                    );
                    None
                }
            }
        };
        match pool {
            Some(pool) => {
                let products = roster
                    .currencies
                    .iter()
                    .filter_map(|c| fx_store::fx_product_id(c))
                    .collect();
                rt.block_on(FxStoreSource::new("store:fx", pool, products).latest())
                    .unwrap_or_else(|e| {
                        eprintln!("[dry-run] the market-data store did not answer: {e}");
                        Vec::new()
                    })
            }
            // Whichever arm produced the `None` has already said why.
            None => Vec::new(),
        }
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
         coingecko {} ids, coinmarketcap {} ids, fx {} currencies, \
         erapi {} currencies, fx-store {} rows",
        pyth.len(),
        coinbase.len(),
        kraken.len(),
        cg.len(),
        cmc.len(),
        fx.len(),
        erapi.len(),
        fx_store_rows.len()
    );
    if !args.drop.is_empty() {
        println!("Suppressed tiers: {}", args.drop.join(", "));
    }
    // A dry run still POLLS a parked venue, unlike the live path which does not
    // spawn it: this is the wiring check, so "has it come back, does my
    // self-hosted instance answer" is exactly the question being asked, and a
    // check that declines to check is worthless. What the live path avoids is
    // the per-poll warning of a permanent poller, which one deliberate poll
    // does not have. Naming the park here is what keeps an empty tier from
    // reading as a fault — the same ambiguity, in the surface where a zero is
    // printed rather than rendered.
    for park in PARKED_SOURCES {
        // Same field order as the live path's `[feed]` line: the venue is the
        // subject of the sentence, so it leads.
        println!(
            "Parked by decision: {} since {} — {}",
            park.venue, park.since, park.reason
        );
    }
    // Column widths fit the longest value each can take: `Unverified` for
    // health, and a pinned basis rendered as `1.0000 pinned`.
    println!(
        "\n  market      mid (USDC)    anchor         health       \
         basis           fx sources                    basis sources"
    );

    let now = Duration::from_secs(0);
    let q = |v: Option<f64>| v.map(|v| Reading::new(v, now));
    // The wall clock, for the one tier whose age the dry run computes rather
    // than assumes: store rows carry their own publication stamp, so they can
    // be aged truthfully where a polled tier cannot.
    let dry_run_now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    /// One leg's consensus, rendered for the dry-run table. A dry run is the
    /// wiring check, so how many sources answered and which one disagrees is
    /// exactly what it exists to show — naming the outlier is the difference
    /// between "something is wrong" and "this id is wrong".
    /// Switches on `state`, not on whether an outlier happens to be named: a
    /// dispersed leg that cannot single out a suspect must not render as
    /// "agree". The state is the authority on what the leg concluded; the
    /// outlier is an optional detail hanging off it.
    fn describe_leg(leg: &LegReport) -> String {
        let n = leg.n;
        match leg.state {
            ConsensusState::Absent => "—".to_string(),
            ConsensusState::Dispersed => match leg.outlier {
                Some(who) => format!("{n} src, {who} out"),
                None => format!("{n} src, DISAGREE"),
            },
            ConsensusState::SingleUnverified => "1 src, unchecked".to_string(),
            ConsensusState::SingleTrusted => "1 src, trusted".to_string(),
            ConsensusState::Agreed | ConsensusState::Corroborated => {
                format!("{n} src, agree")
            }
        }
    }
    // USDC/USD common-mode leg, shared by every market: Kraken's market print,
    // falling back to the CoinGecko index.
    let usdc_q = Candidates::none()
        .push(SOURCE_KRAKEN, q(kraken.get(USDC_KRAKEN_PAIR).copied()))
        .push(SOURCE_COINGECKO, q(cg.get(USDC_COINGECKO_ID).copied()));
    for &m in &markets {
        // FX anchor: Pyth carries its confidence half-width and is the source
        // designated believable on its own; the ECB reference corroborates it.
        // A dry run has no wall-clock history, so every reading is age zero and
        // `pyth_reading`'s publish-time ageing has nothing to bite on — the
        // point here is which sources answered, not staleness.
        let fx_pyth = pyth.get(m.currency).map(|p| match p.confidence {
            Some(conf) => Reading::with_confidence(p.value, now, conf),
            None => Reading::new(p.value, now),
        });
        // The store's intraday tapes, then the reference class — the same
        // order and the same designations as `FeedHub::legs`, which the two
        // collections must agree on or a dry run stops predicting the live mid.
        let mut fx_q = Candidates::none().push_trusted(SOURCE_PYTH, fx_pyth);
        if let Some(product) = fx_store::fx_product_id(m.currency) {
            for source in fx_store::FX_STORE_SOURCES {
                // Aged honestly from the row's own publication stamp, NOT at
                // the age-zero convention the other tiers use here. That
                // convention is harmless for a tier whose freshness the dry
                // run does not adjudicate, and harmful for this one: the HALT
                // column below turns on which sources actually contribute,
                // contribution turns on staleness, and a forced age of zero
                // means every offered row always survives — so the dry run
                // could never reproduce a staleness-driven halt the live bot
                // would take. The receipt age is genuinely ~0 (these rows
                // were just fetched); the publication age is the real signal.
                let reading = fx_store_rows
                    .iter()
                    .find(|r| r.source == source && r.product_id == product)
                    .and_then(|r| {
                        fx_store::store_reading(r.close, r.published_at, dry_run_now_unix, now)
                    });
                // The same helper the live path uses. Writing the dispatch
                // out longhand here is what made "the two collections must
                // agree" a claim in two comments and an invariant in neither.
                fx_q = fx_store::push_store_candidate(fx_q, source, reading);
            }
        }
        let fx_q = fx_q
            .push_reference(SOURCE_FRANKFURTER, q(fx.get(m.currency).copied()))
            .push_reference(SOURCE_ERAPI, q(erapi.get(m.currency).copied()));
        // Basis leg: Coinbase token/USDC, Kraken token/USD, then the reflexive
        // CoinGecko / CMC index. Kraken's USD quote is converted with the peg
        // leg's consensus, exactly as `FeedHub::legs` does — the two collections
        // must agree or a dry run stops predicting the live mid.
        let usdc_per_usd = usdc_q
            .resolve(cfg.fair_value.leg_stale, cfg.fair_value.leg_dispersion_frac)
            .reading
            .map(|r| r.value)
            .filter(|v| *v > 0.0);
        // Only a candidate when the peg leg resolved — see `FeedHub::legs`, whose
        // reasoning this mirrors: an unconverted token/USD print beside a
        // token/USDC one is a unit mismatch masquerading as a disagreement.
        let kraken_q = m.kraken_pair.zip(usdc_per_usd).and_then(|(p, peg)| {
            q(kraken.get(p).copied()).map(|r| Reading::new(r.value / peg, now))
        });
        let basis_q = Candidates::none()
            .push(
                SOURCE_COINBASE,
                m.coinbase_product.and_then(|p| q(coinbase.get(p).copied())),
            )
            .push(SOURCE_KRAKEN, kraken_q)
            .push(
                SOURCE_COINGECKO,
                q(m.coingecko_id.and_then(|id| cg.get(id)).copied()),
            )
            .push(
                SOURCE_CMC,
                q(m.coinmarketcap_id.and_then(|id| cmc.get(&id)).copied()),
            );
        // A fresh engine per row — no history, so no smoothing. The pinned
        // basis is per-market, so it is layered onto the shared calibration
        // here exactly as the live path does in `Context`.
        let legs = build_legs(fx_q, basis_q, usdc_q, m.static_usd);
        let mut engine = FairValueEngine::new(cfg.fair_value.with_pinned_basis(m.pinned_basis));
        // A dry run reports the in-session composition; it is a wiring check,
        // not a clock simulation.
        let fair = engine.compose(legs, now, ClockCtx::in_session());
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
        let mut fx_col = describe_leg(&fair.fx_leg);
        if fair.uncertain {
            fx_col.push_str(" (wide)");
        }
        // The fail-closed verdict, which the leg description alone cannot
        // give. Without this the table is actively misleading in exactly the
        // situation the halt exists for: with the store unreachable, an MVP
        // pair still renders as "2 src, agree" off the daily references, and
        // a reader concludes it is fine when the live bot would refuse to
        // quote it. A dry run that gets the live decision wrong is worse
        // than one that omits it, and this file's own rule is that the two
        // collections must agree.
        //
        // Rendered on the FX column because that is the leg the guard is
        // about, and a whole column would be blank on every healthy row.
        // Same predicate the tick loop uses, via the same helper — a dry run
        // that computed this independently could disagree with the live
        // decision, which is the one thing it exists not to do.
        if tasks::tape_shortfall_for_dry_run(m.requires_live_tape, &fair.fx_leg) {
            fx_col = format!("HALT: no tape ({fx_col})");
        }
        println!(
            "  {:<10}  {:>12}  {:<13}  {:<11}  {:<14}  {:<28}  {}",
            m.symbol,
            mid,
            anchor,
            health,
            basis,
            fx_col,
            describe_leg(&fair.crypto_leg),
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
    /// most roster currencies are published as `USD/<ccy>` and only EUR, GBP
    /// and AUD the other way, so the direction is a per-market fact.
    #[test]
    fn pyth_feeds_carry_each_markets_own_direction() {
        // AUDD and CADC are here because the doc above now names AUD as one
        // of the three non-inverted crosses, and the two new markets are
        // where that claim is newly true — asserting only EUR and ZAR would
        // leave the diff's own direction claim unexercised.
        let markets = args(&["EURC", "ZARP", "AUDD", "CADC"]).selected();
        let roster = FeedRoster::for_markets(&markets);
        let eur = roster.pyth.iter().find(|f| f.key == "EUR").unwrap();
        let zar = roster.pyth.iter().find(|f| f.key == "ZAR").unwrap();
        let aud = roster.pyth.iter().find(|f| f.key == "AUD").unwrap();
        let cad = roster.pyth.iter().find(|f| f.key == "CAD").unwrap();
        assert!(!eur.invert, "EUR/USD is published direct");
        assert!(zar.invert, "ZAR is published as USD/ZAR");
        assert!(!aud.invert, "AUD/USD is published direct");
        assert!(cad.invert, "CAD is published as USD/CAD");
    }
}
