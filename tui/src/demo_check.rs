//! The pre-demo smoke check — one line per surface of a `make demo` stack
//! that is already up, each pass, fail or a named expected state, with the
//! evidence it was judged on.
//!
//! It reads and never writes: no transaction, no database write, no
//! container started or stopped. The surfaces, in the order an operator
//! would notice them broken:
//!
//! - **collectors** — every collector container running, and each writing
//!   rows to its store table within its own cadence's bound;
//! - **estimator** — a fresh `fair_price` row per published market, with
//!   tGBP-USDC's regime and basis spelled out;
//! - **validator** — the localnet answering and the program deployed at the
//!   localnet id;
//! - **markets** — each roster market's book against its posture: a resting
//!   flipping ladder on a passive market, a live maker on an active one;
//! - **frontend** — the dev server answering, and a tGBP-USDC quote through
//!   `simulate_swap`, the function the frontend's WASM build wraps;
//! - **grafana** — its health endpoint answering.
//!
//! **Keyless is a state, not a failure.** A stack brought up without the
//! OANDA and Twelve Data credentials has no FX tape, so the FX-fed markets
//! halt; the check reads the keyed containers' absence as that mode and says
//! so rather than failing every market it explains.

use crate::accounts::{self, Liveness, MarketView};
use crate::chain;
use crate::cluster::Cluster;
use crate::market::{self, PairConfig, Posture};
use anyhow::{Context, Result};
use dropset_market_data::estimator::PUBLISHED_MARKETS;
use dropset_market_data::fair_price::{FairPriceRow, FairPriceSource};
use dropset_sdk::clock::SlotTime;
use dropset_sdk::layout::{Level, MarketView as SlabView, N_LEVELS};
use dropset_sdk::matching::{simulate_swap, SwapSide};
use dropset_sdk::price::Price;
use solana_client::rpc_client::RpcClient;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// The read-only role the dashboards log in as, on the stack's published port.
pub const DEFAULT_DATABASE_URL: &str = "postgres://dropset_ro:dropset_ro@127.0.0.1:5432/dropset";
pub const FRONTEND_PORT: u16 = 3000;
pub const GRAFANA_PORT: u16 = 3200;
const COMPOSE_FILE: &str = "infra/localnet/docker-compose.yml";

/// The credentialed FX collectors whose absence is keyless mode.
const KEYED_FX: [&str; 2] = ["oanda", "twelvedata"];

/// How long a fair row may age before the estimator reads as stalled: four of
/// its default 15 s ticks.
const FAIR_MAX_AGE_SECS: i64 = 60;

/// The quote size the frontend check simulates, in USDC atoms ($10).
const QUOTE_AMOUNT_IN: u64 = 10_000_000;

/// How long an HTTP probe waits — generous, since a cold `next dev` compiles
/// the page on its first request.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// One collector service, the store table and `source` it writes, and how old
/// its newest row may be at its own cadence.
struct Collector {
    service: &'static str,
    table: &'static str,
    time_column: &'static str,
    source: &'static str,
    max_age_secs: i64,
    keyed: bool,
}

const fn collector(
    service: &'static str,
    candles: bool,
    source: &'static str,
    max_age_secs: i64,
    keyed: bool,
) -> Collector {
    let (table, time_column) = if candles {
        ("cex_prices", "bucket_start")
    } else {
        ("spot_ticks", "observed_at")
    };
    Collector {
        service,
        table,
        time_column,
        source,
        max_age_secs,
        keyed,
    }
}

const MINUTE: i64 = 60;
const DAY: i64 = 24 * 60 * MINUTE;

/// The collectors `make collectors-up` starts. The two daily tiers stamp the
/// vendor's snapshot time rather than the poll's, so their bound is the
/// publication gap (Frankfurter's spans a weekend); alphavantage's is its
/// daily candle.
const COLLECTORS: [Collector; 9] = [
    collector("coinbase", true, "coinbase", 10 * MINUTE, false),
    collector("coinbase-ticker", false, "coinbase", 5 * MINUTE, false),
    collector("kraken", false, "kraken", 5 * MINUTE, false),
    collector("orca", false, "orca", 5 * MINUTE, false),
    collector("erapi", false, "erapi", 2 * DAY, false),
    collector("frankfurter", false, "frankfurter", 4 * DAY, false),
    collector("oanda", true, "oanda", 15 * MINUTE, true),
    collector("twelvedata", true, "twelvedata", 2 * 60 * MINUTE, true),
    collector("alphavantage", true, "alphavantage", 3 * DAY, true),
];

/// One surface's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    /// Not healthy, and not wrong either: a named state the stack is in.
    Expected,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Expected => "EXPECTED",
        }
    }
}

/// One printed line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub surface: String,
    pub verdict: Verdict,
    pub evidence: String,
}

impl Check {
    fn new(surface: impl Into<String>, verdict: Verdict, evidence: impl Into<String>) -> Self {
        Self {
            surface: surface.into(),
            verdict,
            evidence: evidence.into(),
        }
    }
}

impl std::fmt::Display for Check {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:<8}  {:<26}  {}",
            self.verdict.label(),
            self.surface,
            self.evidence
        )
    }
}

/// Where the check looks.
pub struct Targets<'a> {
    pub repo_root: &'a Path,
    pub rpc_url: &'a str,
    pub database_url: &'a str,
}

/// Every surface, in order.
pub fn run(targets: &Targets<'_>) -> Vec<Check> {
    let containers = containers(targets.repo_root);
    let keyless = match &containers {
        Ok(states) => is_keyless(states),
        Err(_) => false,
    };
    let mut checks = Vec::new();
    if keyless {
        checks.push(Check::new(
            "mode",
            Verdict::Expected,
            "keyless — no OANDA / Twelve Data collector, so the FX-fed markets halt",
        ));
    }
    let now = dropset_sdk::time::now_unix();
    let db = Db::connect(targets.database_url);
    checks.extend(collector_checks(&containers, db.as_ref().ok(), now));
    checks.push(match &db {
        Ok(db) => estimator_check(db, now, keyless),
        Err(e) => Check::new("estimator", Verdict::Fail, format!("database: {e:#}")),
    });
    let client = chain::rpc(targets.rpc_url);
    let validator = validator_check(&client);
    let up = validator.verdict == Verdict::Pass;
    checks.push(validator);
    if up {
        checks.extend(market_checks(&client, targets.repo_root, keyless));
        checks.push(quote_check(&client, targets.repo_root));
    }
    checks.push(http_check("frontend", FRONTEND_PORT, "/"));
    checks.push(http_check("grafana", GRAFANA_PORT, "/api/health"));
    checks
}

/// The store, with the one runtime its pool lives on — a pooled connection
/// is bound to the runtime that opened it, so every read goes through this.
struct Db {
    runtime: tokio::runtime::Runtime,
    pool: PgPool,
}

impl Db {
    fn connect(url: &str) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let pool = runtime.block_on(
            PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(3))
                .connect(url),
        )?;
        Ok(Self { runtime, pool })
    }

    /// The newest row `c` has written, in Unix seconds; `Ok(None)` for none.
    fn newest_row(&self, c: &Collector) -> Result<Option<i64>> {
        // Both names are compiled constants, never input.
        let sql = format!(
            "SELECT max({}) FROM {} WHERE source = $1",
            c.time_column, c.table
        );
        let query = sqlx::query_scalar(&sql)
            .bind(c.source)
            .fetch_one(&self.pool);
        Ok(self.runtime.block_on(query)?)
    }

    fn fair_rows(&self, products: Vec<String>) -> Result<Vec<FairPriceRow>> {
        let source = FairPriceSource::new("demo-check", self.pool.clone(), products);
        self.runtime.block_on(source.latest())
    }
}

/// `(service, state)` for every container of the stack, stopped ones
/// included. Both profiles, because a profiled service is invisible to `ps`
/// under a profile that is not enabled.
fn containers(repo_root: &Path) -> Result<Vec<(String, String)>> {
    let out = Command::new("docker")
        .current_dir(repo_root)
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "--profile",
            "fx",
            "--profile",
            "taker",
        ])
        .args(["ps", "--all", "--format", "{{.Service}}\t{{.State}}"])
        .output()
        .context("run docker compose ps")?;
    anyhow::ensure!(
        out.status.success(),
        "docker compose ps: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_ps(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(s, st)| (s.trim().to_string(), st.trim().to_string()))
        .collect()
}

fn state_of<'a>(states: &'a [(String, String)], service: &str) -> Option<&'a str> {
    states
        .iter()
        .find(|(s, _)| s == service)
        .map(|(_, st)| st.as_str())
}

/// Keyless: neither keyed FX collector is running.
fn is_keyless(states: &[(String, String)]) -> bool {
    KEYED_FX
        .iter()
        .all(|s| state_of(states, s) != Some("running"))
}

fn collector_checks(
    containers: &Result<Vec<(String, String)>>,
    db: Option<&Db>,
    now: i64,
) -> Vec<Check> {
    let states = match containers {
        Ok(states) => states,
        Err(e) => return vec![Check::new("collectors", Verdict::Fail, format!("{e:#}"))],
    };
    let keyless = is_keyless(states);
    COLLECTORS
        .iter()
        .map(|c| {
            let newest = db.map(|db| db.newest_row(c));
            collector_verdict(c, state_of(states, c.service), newest, now, keyless)
        })
        .collect()
}

fn collector_verdict(
    c: &Collector,
    state: Option<&str>,
    newest: Option<Result<Option<i64>>>,
    now: i64,
    keyless: bool,
) -> Check {
    let surface = format!("collector {}", c.service);
    match state {
        Some("running") => {}
        None if c.keyed && keyless => {
            return Check::new(surface, Verdict::Expected, "not started — keyless")
        }
        None => return Check::new(surface, Verdict::Fail, "no container"),
        Some(other) => return Check::new(surface, Verdict::Fail, format!("container {other}")),
    }
    let newest = match newest {
        None => return Check::new(surface, Verdict::Fail, "running; database unreachable"),
        Some(Err(e)) => return Check::new(surface, Verdict::Fail, format!("running; {e:#}")),
        Some(Ok(None)) => {
            return Check::new(
                surface,
                Verdict::Fail,
                format!("running; no {} row from `{}`", c.table, c.source),
            )
        }
        Some(Ok(Some(ts))) => ts,
    };
    let age = now.saturating_sub(newest).max(0);
    let evidence = format!(
        "running; newest {} row {} old (bound {})",
        c.table,
        fmt_age(age),
        fmt_age(c.max_age_secs)
    );
    let verdict = if age <= c.max_age_secs {
        Verdict::Pass
    } else if c.keyed && fx_market_closed(now) {
        return Check::new(
            surface,
            Verdict::Expected,
            format!("{evidence} — FX market closed"),
        );
    } else {
        Verdict::Fail
    };
    Check::new(surface, verdict, evidence)
}

/// Whether the FX market is shut for the weekend: Friday 22:00 to Sunday
/// 22:00 UTC, when the keyed venues print no candles.
fn fx_market_closed(now: i64) -> bool {
    let days = now.div_euclid(DAY);
    let hour = now.rem_euclid(DAY) / (60 * MINUTE);
    // 1970-01-01 was a Thursday: 0 = Thursday … 1 = Friday, 2 = Saturday,
    // 3 = Sunday.
    match days.rem_euclid(7) {
        1 => hour >= 22,
        2 => true,
        3 => hour < 22,
        _ => false,
    }
}

fn fmt_age(secs: i64) -> String {
    match secs {
        s if s < 2 * MINUTE => format!("{s} s"),
        s if s < 2 * 60 * MINUTE => format!("{} min", s / MINUTE),
        s if s < 2 * DAY => format!("{} h", s / (60 * MINUTE)),
        s => format!("{} d", s / DAY),
    }
}

fn estimator_check(db: &Db, now: i64, keyless: bool) -> Check {
    let products: Vec<String> = PUBLISHED_MARKETS
        .iter()
        .map(|m| m.product_id.to_string())
        .collect();
    match db.fair_rows(products.clone()) {
        Ok(rows) => fair_verdict(&products, &rows, now, keyless),
        Err(e) => Check::new("estimator", Verdict::Fail, format!("{e:#}")),
    }
}

/// Every published product carries a row younger than [`FAIR_MAX_AGE_SECS`];
/// the evidence spells out tGBP-USDC's composition.
fn fair_verdict(products: &[String], rows: &[FairPriceRow], now: i64, keyless: bool) -> Check {
    let mut missing = Vec::new();
    for product in products {
        match rows.iter().find(|r| &r.product_id == product) {
            Some(r) if now - r.ts <= FAIR_MAX_AGE_SECS => {}
            Some(r) => missing.push(format!("{product} {} old", fmt_age(now - r.ts))),
            None => missing.push(format!("{product} absent")),
        }
    }
    let tgbp = rows.iter().find(|r| r.product_id == "TGBP-USDC").map(|r| {
        let basis = match (r.basis, r.basis_age_secs) {
            (Some(b), Some(age)) => format!("basis {b:.5} ({age} s)"),
            (Some(b), None) => format!("basis {b:.5}"),
            _ => "no basis".to_string(),
        };
        let degrade = r
            .degrade
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        format!(
            "TGBP-USDC {}{degrade}, anchor {}, {basis}",
            r.regime, r.anchor
        )
    });
    let tgbp = tgbp.unwrap_or_else(|| "TGBP-USDC no row".to_string());
    if missing.is_empty() {
        let paused = rows.iter().any(|r| r.regime == "paused");
        let verdict = if paused && !keyless {
            Verdict::Fail
        } else {
            Verdict::Pass
        };
        Check::new(
            "estimator",
            verdict,
            format!("{} markets fresh; {tgbp}", products.len()),
        )
    } else {
        Check::new(
            "estimator",
            Verdict::Fail,
            format!("stale: {}; {tgbp}", missing.join(", ")),
        )
    }
}

fn validator_check(client: &RpcClient) -> Check {
    if let Err(e) = client.get_health() {
        return Check::new("validator", Verdict::Fail, format!("unreachable: {e}"));
    }
    let program = Cluster::Localnet.program_id();
    match client.get_account(&program) {
        Ok(account) if account.executable => {
            let slot = client
                .get_slot()
                .map(|s| format!("slot {s}; "))
                .unwrap_or_default();
            Check::new(
                "validator",
                Verdict::Pass,
                format!("{slot}program {program} deployed"),
            )
        }
        Ok(_) => Check::new(
            "validator",
            Verdict::Fail,
            format!("{program} is not executable"),
        ),
        Err(_) => Check::new(
            "validator",
            Verdict::Fail,
            format!("no program at the localnet id {program} — bootstrap from the TUI"),
        ),
    }
}

/// What the check reads off one market.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BookState {
    asks: usize,
    bids: usize,
    /// Posted rungs, and those that flip into a reverse order on fill.
    rungs: usize,
    flipping: usize,
    liveness: Liveness,
}

fn market_checks(client: &RpcClient, repo_root: &Path, keyless: bool) -> Vec<Check> {
    let head = client.get_slot().ok();
    market::roster(Cluster::Localnet)
        .iter()
        .map(|config| {
            let surface = format!("market {}-USDC", config.base.symbol);
            match book_state(client, repo_root, config, head) {
                Ok(Some(state)) => market_verdict(surface, config.posture, state, keyless),
                Ok(None) => Check::new(surface, Verdict::Fail, "no market or vault on chain"),
                Err(e) => Check::new(surface, Verdict::Fail, format!("{e:#}")),
            }
        })
        .collect()
}

fn market_view(
    client: &RpcClient,
    repo_root: &Path,
    config: &PairConfig,
) -> Result<Option<MarketView>> {
    let (base, quote) = market::pair_mints(repo_root, config)?;
    let address = chain::market_pda(Cluster::Localnet, &base, &quote);
    Ok(accounts::read_market_at(client, Cluster::Localnet, address))
}

fn book_state(
    client: &RpcClient,
    repo_root: &Path,
    config: &PairConfig,
    head: Option<u64>,
) -> Result<Option<BookState>> {
    let Some(view) = market_view(client, repo_root, config)? else {
        return Ok(None);
    };
    let account = client.get_account(&view.address).context("read market")?;
    let slab = SlabView::load(&account.data)
        .map_err(|e| anyhow::anyhow!("market does not decode: {e:?}"))?;
    let Some((_, vault)) = slab.active_vaults().next() else {
        return Ok(None);
    };
    let posted = |side: &[Level; N_LEVELS]| -> (usize, usize) {
        let rungs = side.iter().filter(|l| l.size_bps.get() > 0);
        let flipping = rungs
            .clone()
            .filter(|l| l.flip_spread_ppm.get() > 0)
            .count();
        (rungs.count(), flipping)
    };
    let (ask_rungs, ask_flips) = posted(&vault.profile.asks);
    let (bid_rungs, bid_flips) = posted(&vault.profile.bids);
    Ok(Some(BookState {
        asks: view.asks.len(),
        bids: view.bids.len(),
        rungs: ask_rungs + bid_rungs,
        flipping: ask_flips + bid_flips,
        liveness: accounts::maker_liveness(head, view.leader_quote_slot),
    }))
}

/// A passive market passes on a two-sided resting book whose rungs flip; an
/// active one on a two-sided book its maker stamped recently. Keyless, an
/// empty or unquoted book is the halt that mode causes.
fn market_verdict(surface: String, posture: Posture, s: BookState, keyless: bool) -> Check {
    let book = format!("{} asks / {} bids resting", s.asks, s.bids);
    let two_sided = s.asks > 0 && s.bids > 0;
    let (healthy, evidence) = match posture {
        Posture::Passive => (
            two_sided && s.flipping > 0,
            format!("passive; {book}; {} of {} rungs flip", s.flipping, s.rungs),
        ),
        Posture::Active => (
            two_sided && s.liveness == Liveness::Live,
            format!("active; {book}; maker {}", liveness_label(s.liveness)),
        ),
    };
    let verdict = match (healthy, keyless) {
        (true, _) => Verdict::Pass,
        (false, true) => Verdict::Expected,
        (false, false) => Verdict::Fail,
    };
    let evidence = if verdict == Verdict::Expected {
        format!("{evidence} — halted, keyless")
    } else {
        evidence
    };
    Check::new(surface, verdict, evidence)
}

fn liveness_label(l: Liveness) -> &'static str {
    match l {
        Liveness::Live => "quoting",
        Liveness::Stale => "stale",
        Liveness::Unknown => "never quoted",
    }
}

/// A $10 tGBP buy through [`simulate_swap`] — the function the frontend's
/// WASM build exports, so a quote here is the quote the page would show over
/// the same account.
fn quote_check(client: &RpcClient, repo_root: &Path) -> Check {
    let surface = "quote TGBP-USDC";
    let config = &market::MARKET_TGBP;
    let quote = market_view(client, repo_root, config).and_then(|view| {
        let view = view.context("no market on chain")?;
        let account = client.get_account(&view.address).context("read market")?;
        let slab = SlabView::load(&account.data)
            .map_err(|e| anyhow::anyhow!("market does not decode: {e:?}"))?;
        let slot = client.get_slot().context("slot")?;
        Ok(simulate_swap(
            &slab,
            SwapSide::Buy,
            QUOTE_AMOUNT_IN,
            Price::INFINITY,
            SlotTime::new(slot.min(u64::from(u32::MAX)) as u32),
            dropset_sdk::time::now_unix_u32(),
            0,
        ))
    });
    match quote {
        Ok(q) if q.out_amount > 0 => {
            let out = q.out_amount as f64 / 10f64.powi(i32::from(config.base.decimals));
            Check::new(
                surface,
                Verdict::Pass,
                format!("$10 buys {out:.4} TGBP across {} legs", q.legs),
            )
        }
        Ok(_) => Check::new(
            surface,
            Verdict::Fail,
            "$10 buy fills nothing — the book is empty",
        ),
        Err(e) => Check::new(surface, Verdict::Fail, format!("{e:#}")),
    }
}

/// A plain HTTP/1.0 GET on loopback, passing on a 2xx status.
fn http_check(surface: &str, port: u16, path: &str) -> Check {
    match http_status(port, path) {
        Ok(status) if (200..300).contains(&status) => Check::new(
            surface,
            Verdict::Pass,
            format!("http://localhost:{port}{path} → {status}"),
        ),
        Ok(status) => Check::new(
            surface,
            Verdict::Fail,
            format!("http://localhost:{port}{path} → {status}"),
        ),
        Err(e) => Check::new(
            surface,
            Verdict::Fail,
            format!("http://localhost:{port}{path}: {e:#}"),
        ),
    }
}

fn http_status(port: u16, path: &str) -> Result<u16> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).context("not listening")?;
    stream.set_read_timeout(Some(HTTP_TIMEOUT))?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: localhost:{port}\r\n\r\n"
    )?;
    let mut head = [0u8; 64];
    let n = stream.read(&mut head).context("no response")?;
    parse_status(&String::from_utf8_lossy(&head[..n])).context("not an HTTP response")
}

fn parse_status(head: &str) -> Option<u16> {
    let mut parts = head.split_whitespace();
    parts.next().filter(|v| v.starts_with("HTTP/"))?;
    parts.next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(s, st)| (s.to_string(), st.to_string()))
            .collect()
    }

    fn row(product: &str, ts: i64, regime: &str) -> FairPriceRow {
        FairPriceRow {
            ts,
            product_id: product.to_string(),
            fair: Some(1.3),
            anchor: "fx".to_string(),
            regime: regime.to_string(),
            degrade: None,
            health: "ok".to_string(),
            basis: Some(0.9991),
            basis_age_secs: Some(4),
            basis_outlier: false,
            uncertain: false,
            basis_breach: false,
            usdc_breach: false,
            fx_tape_live: Some(true),
        }
    }

    // A Thursday, 12:00 UTC.
    const THURSDAY_NOON: i64 = 1_792_670_400;

    #[test]
    fn ps_output_parses_service_and_state() {
        let parsed = parse_ps("kraken\trunning\noanda\texited\n\n");
        assert_eq!(
            parsed,
            states(&[("kraken", "running"), ("oanda", "exited")])
        );
    }

    #[test]
    fn keyless_is_both_keyed_fx_collectors_down() {
        assert!(is_keyless(&states(&[("kraken", "running")])));
        assert!(is_keyless(&states(&[("oanda", "exited")])));
        assert!(!is_keyless(&states(&[("oanda", "running")])));
        assert!(!is_keyless(&states(&[("twelvedata", "running")])));
    }

    #[test]
    fn a_collector_passes_within_its_bound_and_fails_past_it() {
        let kraken = &COLLECTORS[2];
        let now = THURSDAY_NOON;
        let fresh = collector_verdict(
            kraken,
            Some("running"),
            Some(Ok(Some(now - 30))),
            now,
            false,
        );
        assert_eq!(fresh.verdict, Verdict::Pass, "{fresh}");
        let stale = collector_verdict(
            kraken,
            Some("running"),
            Some(Ok(Some(now - 600))),
            now,
            false,
        );
        assert_eq!(stale.verdict, Verdict::Fail, "{stale}");
        assert!(stale.evidence.contains("10 min old"), "{stale}");
    }

    #[test]
    fn a_missing_keyed_collector_is_expected_only_when_keyless() {
        let oanda = &COLLECTORS[6];
        let keyless = collector_verdict(oanda, None, None, THURSDAY_NOON, true);
        assert_eq!(keyless.verdict, Verdict::Expected);
        let kraken = &COLLECTORS[2];
        assert_eq!(
            collector_verdict(kraken, None, None, THURSDAY_NOON, true).verdict,
            Verdict::Fail
        );
        assert_eq!(
            collector_verdict(kraken, Some("restarting"), None, THURSDAY_NOON, false).verdict,
            Verdict::Fail
        );
    }

    #[test]
    fn a_stale_keyed_collector_is_expected_over_the_fx_weekend() {
        let oanda = &COLLECTORS[6];
        // Saturday 12:00 UTC, the newest candle from Friday evening.
        let saturday = THURSDAY_NOON + 2 * DAY;
        let c = collector_verdict(
            oanda,
            Some("running"),
            Some(Ok(Some(saturday - 15 * 3600))),
            saturday,
            false,
        );
        assert_eq!(c.verdict, Verdict::Expected, "{c}");
    }

    #[test]
    fn the_fx_weekend_runs_friday_22_to_sunday_22_utc() {
        let friday = THURSDAY_NOON + DAY;
        assert!(!fx_market_closed(THURSDAY_NOON));
        assert!(!fx_market_closed(friday + 9 * 3600)); // Friday 21:00
        assert!(fx_market_closed(friday + 10 * 3600)); // Friday 22:00
        assert!(fx_market_closed(friday + DAY)); // Saturday noon
        assert!(fx_market_closed(friday + 2 * DAY + 9 * 3600)); // Sunday 21:00
        assert!(!fx_market_closed(friday + 2 * DAY + 10 * 3600)); // Sunday 22:00
    }

    #[test]
    fn the_estimator_check_names_the_tgbp_composition() {
        let products = vec!["EURC-USDC".to_string(), "TGBP-USDC".to_string()];
        let now = THURSDAY_NOON;
        let rows = vec![
            row("EURC-USDC", now - 5, "normal"),
            row("TGBP-USDC", now - 5, "normal"),
        ];
        let c = fair_verdict(&products, &rows, now, false);
        assert_eq!(c.verdict, Verdict::Pass, "{c}");
        assert!(
            c.evidence
                .contains("TGBP-USDC normal, anchor fx, basis 0.99910 (4 s)"),
            "{c}"
        );
    }

    #[test]
    fn a_stale_or_absent_fair_fails() {
        let products = vec!["EURC-USDC".to_string(), "TGBP-USDC".to_string()];
        let now = THURSDAY_NOON;
        let rows = vec![row("EURC-USDC", now - 300, "normal")];
        let c = fair_verdict(&products, &rows, now, false);
        assert_eq!(c.verdict, Verdict::Fail);
        assert!(c.evidence.contains("EURC-USDC 5 min old"), "{c}");
        assert!(c.evidence.contains("TGBP-USDC absent"), "{c}");
    }

    #[test]
    fn a_paused_fair_fails_unless_keyless() {
        let products = vec!["TGBP-USDC".to_string()];
        let now = THURSDAY_NOON;
        let rows = vec![row("TGBP-USDC", now - 5, "paused")];
        assert_eq!(
            fair_verdict(&products, &rows, now, false).verdict,
            Verdict::Fail
        );
        assert_eq!(
            fair_verdict(&products, &rows, now, true).verdict,
            Verdict::Pass
        );
    }

    fn book(asks: usize, bids: usize, flipping: usize, liveness: Liveness) -> BookState {
        BookState {
            asks,
            bids,
            rungs: 8,
            flipping,
            liveness,
        }
    }

    #[test]
    fn a_passive_market_needs_a_resting_flipping_ladder_not_a_live_maker() {
        let ok = market_verdict(
            "m".into(),
            Posture::Passive,
            book(4, 4, 8, Liveness::Stale),
            false,
        );
        assert_eq!(ok.verdict, Verdict::Pass, "{ok}");
        assert!(ok.evidence.contains("8 of 8 rungs flip"), "{ok}");
        let no_flip = market_verdict(
            "m".into(),
            Posture::Passive,
            book(4, 4, 0, Liveness::Live),
            false,
        );
        assert_eq!(no_flip.verdict, Verdict::Fail);
    }

    #[test]
    fn an_active_market_needs_a_live_maker() {
        let ok = market_verdict(
            "m".into(),
            Posture::Active,
            book(4, 4, 0, Liveness::Live),
            false,
        );
        assert_eq!(ok.verdict, Verdict::Pass);
        let stale = market_verdict(
            "m".into(),
            Posture::Active,
            book(4, 4, 0, Liveness::Stale),
            false,
        );
        assert_eq!(stale.verdict, Verdict::Fail);
        assert!(stale.evidence.contains("maker stale"), "{stale}");
    }

    #[test]
    fn an_empty_book_is_the_keyless_halt_not_a_failure() {
        let c = market_verdict(
            "m".into(),
            Posture::Passive,
            book(0, 0, 0, Liveness::Unknown),
            true,
        );
        assert_eq!(c.verdict, Verdict::Expected);
        assert!(c.evidence.ends_with("halted, keyless"), "{c}");
    }

    #[test]
    fn the_http_status_line_parses() {
        assert_eq!(parse_status("HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(parse_status("HTTP/1.0 503 Service Unavailable"), Some(503));
        assert_eq!(parse_status("SSH-2.0-OpenSSH"), None);
    }
}
