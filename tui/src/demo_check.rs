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
//! - **quote** — a tGBP-USDC quote through `simulate_swap`, the function the
//!   frontend's WASM build wraps, over the live market account. Chain-side
//!   only: the page itself is not driven;
//! - **frontend** and **grafana** — each server answering.
//!
//! With the validator down, the market and quote lines print as skipped
//! rather than vanish or fail a second time, so the count of surfaces is
//! the same on every run.
//!
//! **Keyless is a state, not a failure.** A stack brought up without the
//! OANDA and Twelve Data credentials has no FX tape, so the FX-fed markets
//! halt; the check reads the keyed containers' absence as that mode and
//! excuses exactly the halt it causes — an empty book, a paused fair —
//! while anything else wrong on a market still fails.

use crate::accounts::{self, Liveness, MarketView};
use crate::chain;
use crate::cluster::Cluster;
use crate::market::{self, PairConfig, Posture};
use anyhow::{anyhow, Context, Result};
use dropset_market_data::estimator::PUBLISHED_MARKETS;
use dropset_market_data::fair_price::{FairPriceRow, FairPriceSource};
use dropset_sdk::clock::SlotTime;
use dropset_sdk::layout::{Level, MarketView as SlabView, N_LEVELS};
use dropset_sdk::matching::{simulate_swap, SwapSide};
use dropset_sdk::price::Price;
use solana_client::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// The read-only role the dashboards log in as, on the stack's published port.
pub const DEFAULT_DATABASE_URL: &str = "postgres://dropset_ro:dropset_ro@127.0.0.1:5432/dropset";

/// The `next dev` port `make frontend-localnet` serves on.
const FRONTEND_PORT: u16 = 3000;

/// Grafana's loopback host port in the compose file.
const GRAFANA_PORT: u16 = 3200;

/// The compose file every stack service is defined in, from the repo root.
const COMPOSE_FILE: &str = "infra/localnet/docker-compose.yml";

/// The credentialed FX collectors whose absence is keyless mode.
const KEYED_FX: [&str; 2] = ["oanda", "twelvedata"];

/// How long a fair row may age before the estimator reads as stalled: four of
/// its default 15 s ticks.
const FAIR_MAX_AGE_SECS: i64 = 60;

/// The quote size the quote check simulates, in USDC atoms ($10).
const QUOTE_AMOUNT_IN: u64 = 10_000_000;

/// How long an HTTP probe waits — generous, since a cold `next dev` compiles
/// the page on its first request.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one database read may take, connect included.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// The store table a collector writes, which fixes its time column.
#[derive(Clone, Copy)]
enum Table {
    /// `cex_prices`, aged by `bucket_start`.
    Candles,
    /// `spot_ticks`, aged by `observed_at`.
    Ticks,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Table::Candles => "cex_prices",
            Table::Ticks => "spot_ticks",
        }
    }

    fn time_column(self) -> &'static str {
        match self {
            Table::Candles => "bucket_start",
            Table::Ticks => "observed_at",
        }
    }
}

/// One collector service, the store table and `source` it writes, and how old
/// its newest row may be at its own cadence.
struct Collector {
    service: &'static str,
    table: Table,
    source: &'static str,
    max_age_secs: i64,
    /// Started only under the `fx` profile, with credentials.
    keyed: bool,
}

/// Every collector a demo stack can run: the keyless ones `make
/// collectors-up` always starts, and the keyed three it starts under the `fx`
/// profile when the secrets enclave exists. The two daily tiers stamp the
/// vendor's snapshot time rather than the poll's, so their bound is the
/// publication gap (Frankfurter's spans a weekend); alphavantage's is its
/// daily candle.
const COLLECTORS: [Collector; 9] = [
    Collector {
        service: "coinbase",
        table: Table::Candles,
        source: "coinbase",
        max_age_secs: 10 * MINUTE,
        keyed: false,
    },
    Collector {
        service: "coinbase-ticker",
        table: Table::Ticks,
        source: "coinbase",
        max_age_secs: 5 * MINUTE,
        keyed: false,
    },
    Collector {
        service: "kraken",
        table: Table::Ticks,
        source: "kraken",
        max_age_secs: 5 * MINUTE,
        keyed: false,
    },
    Collector {
        service: "orca",
        table: Table::Ticks,
        source: "orca",
        max_age_secs: 5 * MINUTE,
        keyed: false,
    },
    Collector {
        service: "erapi",
        table: Table::Ticks,
        source: "erapi",
        max_age_secs: 2 * DAY,
        keyed: false,
    },
    Collector {
        service: "frankfurter",
        table: Table::Ticks,
        source: "frankfurter",
        max_age_secs: 4 * DAY,
        keyed: false,
    },
    Collector {
        service: "oanda",
        table: Table::Candles,
        source: "oanda",
        max_age_secs: 15 * MINUTE,
        keyed: true,
    },
    Collector {
        service: "twelvedata",
        table: Table::Candles,
        source: "twelvedata",
        max_age_secs: 2 * HOUR,
        keyed: true,
    },
    Collector {
        service: "alphavantage",
        table: Table::Candles,
        source: "alphavantage",
        max_age_secs: 3 * DAY,
        keyed: true,
    },
];

/// One surface's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The surface does what the demo needs of it.
    Pass,
    /// It does not, and the run exits non-zero.
    Fail,
    /// Not healthy, and not wrong either: a named state the stack is in.
    Expected,
    /// Not checked, because a surface it reads through already failed —
    /// neither a pass nor a second fail for the one fault.
    Skipped,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Expected => "EXPECTED",
            Verdict::Skipped => "SKIPPED",
        }
    }
}

/// Printed before the surfaces: every line reads a live stack, so a run
/// against one still bootstrapping, or with its makers not started, fails
/// lines that are not broken.
pub const HEADER: &str = "against the live demo: run from a second terminal while `make demo` \
                          is up, after `Bootstrap complete` and M";

/// The closing line, and whether the run passed: a fail makes it non-zero,
/// and a skip is named so a down stack is not read as partially passing.
pub fn summary(checks: &[Check]) -> (bool, String) {
    let failed = checks.iter().filter(|c| c.verdict == Verdict::Fail).count();
    let skipped: Vec<&str> = checks
        .iter()
        .filter(|c| c.verdict == Verdict::Skipped)
        .map(|c| c.surface.as_str())
        .collect();
    let skipped = match skipped.len() {
        0 => String::new(),
        n => format!("; {n} skipped ({})", skipped.join(", ")),
    };
    if failed == 0 && skipped.is_empty() {
        return (true, "demo stack ready".into());
    }
    (
        failed == 0,
        format!("{failed} surface(s) failed{skipped} — see docs/demo.md for what each line means"),
    )
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
    let mut checks = Vec::new();
    let now = dropset_sdk::time::now_unix();
    let db = Db::connect(targets.database_url);
    let keyless = match containers(targets.repo_root) {
        Ok(states) => {
            let keyless = is_keyless(&states);
            if keyless {
                checks.push(Check::new(
                    "mode",
                    Verdict::Expected,
                    "keyless — no OANDA / Twelve Data container, so the FX-fed markets halt",
                ));
            }
            checks.extend(collector_checks(&states, keyless, db.as_ref().ok(), now));
            keyless
        }
        Err(e) => {
            checks.push(Check::new("collectors", Verdict::Fail, format!("{e:#}")));
            false
        }
    };
    checks.push(match &db {
        Ok(db) => estimator_check(db, now, keyless),
        Err(e) => Check::new("estimator", Verdict::Fail, format!("database: {e:#}")),
    });
    let client = chain::rpc(targets.rpc_url);
    let validator = validator_check(&client);
    if validator.verdict == Verdict::Pass {
        checks.push(validator);
        checks.extend(market_checks(&client, targets.repo_root, keyless));
        checks.push(quote_check(&client, targets.repo_root, keyless));
    } else {
        checks.extend(chain_skipped(validator));
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
        let connect = PgPoolOptions::new().max_connections(1).connect(url);
        let pool = Self::bounded(&runtime, async { Ok(connect.await?) })?;
        Ok(Self { runtime, pool })
    }

    /// Run `read` inside [`READ_TIMEOUT`], so a hung query fails its line
    /// rather than the whole run.
    fn bounded<T>(
        runtime: &tokio::runtime::Runtime,
        read: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        runtime.block_on(async {
            tokio::time::timeout(READ_TIMEOUT, read)
                .await
                .context("timed out")?
        })
    }

    /// The newest row `c` has written, in Unix seconds; `Ok(None)` for none.
    fn newest_row(&self, c: &Collector) -> Result<Option<i64>> {
        // Both names are compiled constants, never input.
        let sql = format!(
            "SELECT max({}) FROM {} WHERE source = $1",
            c.table.time_column(),
            c.table.name()
        );
        let query = sqlx::query_scalar(&sql)
            .bind(c.source)
            .fetch_one(&self.pool);
        Self::bounded(&self.runtime, async { Ok(query.await?) })
    }

    fn fair_rows(&self, products: Vec<String>) -> Result<Vec<FairPriceRow>> {
        let source = FairPriceSource::new("demo-check", self.pool.clone(), products);
        Self::bounded(&self.runtime, source.latest())
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

/// Keyless: neither keyed FX collector has a container at all. A container
/// that exists but has stopped is a keyed stack whose collector died, which
/// its own line fails — it must not excuse the markets as keyless.
fn is_keyless(states: &[(String, String)]) -> bool {
    KEYED_FX.iter().all(|s| state_of(states, s).is_none())
}

fn collector_checks(
    states: &[(String, String)],
    keyless: bool,
    db: Option<&Db>,
    now: i64,
) -> Vec<Check> {
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
                format!("running; no {} row from `{}`", c.table.name(), c.source),
            )
        }
        Some(Ok(Some(ts))) => ts,
    };
    let age = now.saturating_sub(newest).max(0);
    let evidence = format!(
        "running; newest {} row {} old (bound {})",
        c.table.name(),
        fmt_age(age),
        fmt_age(c.max_age_secs)
    );
    if age <= c.max_age_secs {
        return Check::new(surface, Verdict::Pass, evidence);
    }
    // Over the weekend a keyed venue prints nothing, so its newest candle is
    // the close's — excused only if it is that recent.
    match fx_weekend_close(now) {
        Some(close) if c.keyed && newest >= close - c.max_age_secs => Check::new(
            surface,
            Verdict::Expected,
            format!("{evidence} — FX market closed"),
        ),
        _ => Check::new(surface, Verdict::Fail, evidence),
    }
}

/// When `now` falls in the FX weekend, the Friday close it follows. The
/// window is Friday 21:00 to Sunday 22:00 UTC — the union of the summer and
/// winter sessions (the close and the reopen each move an hour with New York
/// daylight time), so a closed market is never failed; the price is that a
/// keyed collector stalled in the hour either side reads as closed.
fn fx_weekend_close(now: i64) -> Option<i64> {
    let midnight = now - now.rem_euclid(DAY);
    let hour = now.rem_euclid(DAY) / HOUR;
    // 1970-01-01 was a Thursday: 1 = Friday, 2 = Saturday, 3 = Sunday.
    let friday_midnight = match now.div_euclid(DAY).rem_euclid(7) {
        1 if hour >= 21 => midnight,
        2 => midnight - DAY,
        3 if hour < 22 => midnight - 2 * DAY,
        _ => return None,
    };
    Some(friday_midnight + 21 * HOUR)
}

fn fmt_age(secs: i64) -> String {
    match secs {
        s if s < 2 * MINUTE => format!("{s} s"),
        s if s < 2 * HOUR => format!("{} min", s / MINUTE),
        s if s < 2 * DAY => format!("{} h", s / HOUR),
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
/// the evidence spells out tGBP-USDC's composition. A `degraded` regime still
/// passes — the line names it so the operator can say why. A `paused` one is
/// the keyless halt when keyless, and a failure otherwise.
fn fair_verdict(products: &[String], rows: &[FairPriceRow], now: i64, keyless: bool) -> Check {
    let mut missing = Vec::new();
    for product in products {
        match rows.iter().find(|r| &r.product_id == product) {
            Some(r) if now.saturating_sub(r.ts) <= FAIR_MAX_AGE_SECS => {}
            Some(r) => missing.push(format!(
                "{product} {} old",
                fmt_age(now.saturating_sub(r.ts).max(0))
            )),
            None => missing.push(format!("{product} absent")),
        }
    }
    let tgbp = rows
        .iter()
        .find(|r| r.product_id == "TGBP-USDC")
        .map_or_else(|| "TGBP-USDC no row".to_string(), describe_fair);
    if !missing.is_empty() {
        return Check::new(
            "estimator",
            Verdict::Fail,
            format!("stale: {}; {tgbp}", missing.join(", ")),
        );
    }
    let paused: Vec<&str> = rows
        .iter()
        .filter(|r| r.regime == "paused")
        .map(|r| r.product_id.as_str())
        .collect();
    let fresh = format!("{} markets fresh; {tgbp}", products.len());
    match (paused.is_empty(), keyless) {
        (true, _) => Check::new("estimator", Verdict::Pass, fresh),
        (false, true) => Check::new(
            "estimator",
            Verdict::Expected,
            format!("{fresh}; paused {} — keyless", paused.join(", ")),
        ),
        (false, false) => Check::new(
            "estimator",
            Verdict::Fail,
            format!("{fresh}; paused {}", paused.join(", ")),
        ),
    }
}

fn describe_fair(r: &FairPriceRow) -> String {
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
        "{} {}{degrade}, anchor {}, {basis}",
        r.product_id, r.regime, r.anchor
    )
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

const QUOTE_SURFACE: &str = "quote TGBP-USDC";

fn market_surface(config: &PairConfig) -> String {
    format!("market {}-USDC", config.base.symbol)
}

/// A failed validator line, then every line read through the chain as
/// skipped with its reason — the surface count stays stable, and the one
/// fault fails once.
fn chain_skipped(validator: Check) -> Vec<Check> {
    let reason = format!("not checked — validator failed: {}", validator.evidence);
    let surfaces = market::roster(Cluster::Localnet)
        .iter()
        .map(|config| market_surface(config))
        .chain([QUOTE_SURFACE.to_string()]);
    std::iter::once(validator)
        .chain(surfaces.map(|s| Check::new(s, Verdict::Skipped, reason.clone())))
        .collect()
}

/// The head slot is read once and required: without it `maker_liveness`
/// reads every vault `Unknown`, which the label would misreport as no quote
/// on chain.
fn market_checks(client: &RpcClient, repo_root: &Path, keyless: bool) -> Vec<Check> {
    let head = client.get_slot();
    market::roster(Cluster::Localnet)
        .iter()
        .map(|config| {
            let surface = market_surface(config);
            let head = match &head {
                Ok(slot) => *slot,
                Err(e) => return Check::new(surface, Verdict::Fail, format!("head slot: {e}")),
            };
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

/// The market account's raw data — [`MarketView`] keeps only what it decoded,
/// and both the profile read and the quote need the slab itself.
fn market_data(client: &RpcClient, address: &Pubkey) -> Result<Vec<u8>> {
    Ok(client.get_account(address).context("read market")?.data)
}

fn load_slab(data: &[u8]) -> Result<SlabView<'_>> {
    SlabView::load(data).map_err(|e| anyhow!("market does not decode: {e:?}"))
}

fn book_state(
    client: &RpcClient,
    repo_root: &Path,
    config: &PairConfig,
    head: u64,
) -> Result<Option<BookState>> {
    let Some(view) = market_view(client, repo_root, config)? else {
        return Ok(None);
    };
    let data = market_data(client, &view.address)?;
    let slab = load_slab(&data)?;
    let Some((_, vault)) = slab.active_vaults().next() else {
        return Ok(None);
    };
    let (ask_rungs, ask_flips) = rung_counts(&vault.profile.asks);
    let (bid_rungs, bid_flips) = rung_counts(&vault.profile.bids);
    Ok(Some(BookState {
        asks: view.asks.len(),
        bids: view.bids.len(),
        rungs: ask_rungs + bid_rungs,
        flipping: ask_flips + bid_flips,
        liveness: accounts::maker_liveness(Some(head), view.leader_quote_slot),
    }))
}

/// `(posted, flipping)` rungs on one side of a profile: posted is a non-zero
/// size, and a posted rung flips when its flip spread is non-zero.
fn rung_counts(side: &[Level; N_LEVELS]) -> (usize, usize) {
    let posted = side.iter().filter(|l| l.size_bps.get() > 0);
    let flipping = posted
        .clone()
        .filter(|l| l.flip_spread_ppm.get() > 0)
        .count();
    (posted.count(), flipping)
}

/// A passive market passes on a two-sided resting book whose rungs flip; an
/// active one on a two-sided book its maker stamped recently. Keyless, an
/// empty book is the halt that mode causes; anything else wrong still fails.
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
            format!("active; {book}; {}", liveness_label(s.liveness)),
        ),
    };
    let halted = s.asks == 0 && s.bids == 0;
    match (healthy, keyless && halted) {
        (true, _) => Check::new(surface, Verdict::Pass, evidence),
        (false, true) => Check::new(
            surface,
            Verdict::Expected,
            format!("{evidence} — halted, keyless"),
        ),
        (false, false) => Check::new(surface, Verdict::Fail, evidence),
    }
}

/// Read off the vault's quote slot on chain, never off which makers the TUI
/// started: a zero slot is a maker not started and one started but not yet
/// quoting alike, so it reads "yet", and a re-run settles the second.
fn liveness_label(l: Liveness) -> &'static str {
    match l {
        Liveness::Live => "maker quoting",
        Liveness::Stale => "maker stale",
        Liveness::Unknown => "no quote on chain yet (start the makers with M)",
    }
}

/// A $10 tGBP buy through [`simulate_swap`] — the function the frontend's
/// WASM build exports, over the same account, with no platform fee. It
/// proves the chain-side quote; the page itself is not driven.
fn quote_check(client: &RpcClient, repo_root: &Path, keyless: bool) -> Check {
    let config = &market::MARKET_TGBP;
    let quote = market_view(client, repo_root, config).and_then(|view| {
        let view = view.context("no market on chain")?;
        let data = market_data(client, &view.address)?;
        let slab = load_slab(&data)?;
        let slot = client.get_slot().context("slot")?;
        Ok(simulate_swap(
            &slab,
            SwapSide::Buy,
            QUOTE_AMOUNT_IN,
            Price::INFINITY,
            SlotTime::new(u32::try_from(slot).unwrap_or(u32::MAX)),
            dropset_sdk::time::now_unix_u32(),
            0,
        ))
    });
    match quote {
        Ok(q) => quote_verdict(q.out_amount, q.legs, config.base.decimals, keyless),
        Err(e) => Check::new(QUOTE_SURFACE, Verdict::Fail, format!("{e:#}")),
    }
}

/// A fill passes; an empty one is the keyless halt when keyless, and a
/// failure otherwise.
fn quote_verdict(out_amount: u64, legs: u32, base_decimals: u8, keyless: bool) -> Check {
    let surface = QUOTE_SURFACE;
    if out_amount > 0 {
        let out = out_amount as f64 / 10f64.powi(i32::from(base_decimals));
        return Check::new(
            surface,
            Verdict::Pass,
            format!("$10 buys {out:.4} TGBP across {legs} legs (chain-side simulate_swap)"),
        );
    }
    let empty = "$10 buy fills nothing — the book is empty";
    if keyless {
        Check::new(
            surface,
            Verdict::Expected,
            format!("{empty} — halted, keyless"),
        )
    } else {
        Check::new(surface, Verdict::Fail, empty)
    }
}

/// A plain HTTP/1.0 GET on loopback, passing on any 2xx or 3xx — a redirect
/// is the server answering.
fn http_check(surface: &str, port: u16, path: &str) -> Check {
    let url = format!("http://localhost:{port}{path}");
    match http_status(port, path) {
        Ok(status) if (200..400).contains(&status) => {
            Check::new(surface, Verdict::Pass, format!("{url} → {status}"))
        }
        Ok(status) => Check::new(surface, Verdict::Fail, format!("{url} → {status}")),
        Err(e) => Check::new(surface, Verdict::Fail, format!("{url}: {e:#}")),
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
    use dropset_sdk::layout::Level;

    fn states(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(s, st)| (s.to_string(), st.to_string()))
            .collect()
    }

    fn collector(service: &str) -> &'static Collector {
        COLLECTORS
            .iter()
            .find(|c| c.service == service)
            .expect("a rostered collector")
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
    const FRIDAY_MIDNIGHT: i64 = THURSDAY_NOON + 12 * HOUR;

    #[test]
    fn ps_output_parses_service_and_state() {
        let parsed = parse_ps("kraken\trunning\noanda\texited\n\n");
        assert_eq!(
            parsed,
            states(&[("kraken", "running"), ("oanda", "exited")])
        );
    }

    #[test]
    fn keyless_is_both_keyed_fx_containers_absent() {
        assert!(is_keyless(&states(&[("kraken", "running")])));
        assert!(!is_keyless(&states(&[("oanda", "running")])));
        assert!(!is_keyless(&states(&[("twelvedata", "running")])));
    }

    #[test]
    fn a_crashed_keyed_collector_is_not_keyless_mode() {
        let crashed = states(&[("oanda", "exited"), ("twelvedata", "restarting")]);
        assert!(!is_keyless(&crashed));
        let c = collector_verdict(
            collector("oanda"),
            Some("exited"),
            None,
            THURSDAY_NOON,
            false,
        );
        assert_eq!(c.verdict, Verdict::Fail, "{c}");
    }

    #[test]
    fn a_collector_passes_within_its_bound_and_fails_past_it() {
        let kraken = collector("kraken");
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
    fn a_collector_fails_without_a_database_or_a_row() {
        let kraken = collector("kraken");
        let now = THURSDAY_NOON;
        let unreachable = collector_verdict(kraken, Some("running"), None, now, false);
        assert!(
            unreachable.evidence.contains("database unreachable"),
            "{unreachable}"
        );
        let errored = collector_verdict(
            kraken,
            Some("running"),
            Some(Err(anyhow!("boom"))),
            now,
            false,
        );
        assert_eq!(errored.verdict, Verdict::Fail);
        let empty = collector_verdict(kraken, Some("running"), Some(Ok(None)), now, false);
        assert!(
            empty.evidence.contains("no spot_ticks row from `kraken`"),
            "{empty}"
        );
    }

    #[test]
    fn a_missing_keyed_collector_is_expected_only_when_keyless() {
        let keyless = collector_verdict(collector("oanda"), None, None, THURSDAY_NOON, true);
        assert_eq!(keyless.verdict, Verdict::Expected);
        let kraken = collector("kraken");
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
    fn a_keyed_collector_is_excused_over_the_weekend_only_back_to_the_close() {
        let oanda = collector("oanda");
        let saturday_noon = FRIDAY_MIDNIGHT + DAY + 12 * HOUR;
        let close = FRIDAY_MIDNIGHT + 21 * HOUR;
        let at_close = collector_verdict(
            oanda,
            Some("running"),
            Some(Ok(Some(close - 60))),
            saturday_noon,
            false,
        );
        assert_eq!(at_close.verdict, Verdict::Expected, "{at_close}");
        // Stuck since Wednesday: the weekend does not excuse it.
        let wednesday = FRIDAY_MIDNIGHT - 2 * DAY;
        let stuck = collector_verdict(
            oanda,
            Some("running"),
            Some(Ok(Some(wednesday))),
            saturday_noon,
            false,
        );
        assert_eq!(stuck.verdict, Verdict::Fail, "{stuck}");
    }

    #[test]
    fn the_fx_weekend_runs_friday_21_to_sunday_22_utc() {
        let close = FRIDAY_MIDNIGHT + 21 * HOUR;
        assert_eq!(fx_weekend_close(THURSDAY_NOON), None);
        assert_eq!(fx_weekend_close(FRIDAY_MIDNIGHT + 20 * HOUR), None);
        assert_eq!(fx_weekend_close(close), Some(close));
        assert_eq!(
            fx_weekend_close(FRIDAY_MIDNIGHT + DAY + 12 * HOUR),
            Some(close)
        );
        assert_eq!(
            fx_weekend_close(FRIDAY_MIDNIGHT + 2 * DAY + 21 * HOUR),
            Some(close)
        );
        assert_eq!(
            fx_weekend_close(FRIDAY_MIDNIGHT + 2 * DAY + 22 * HOUR),
            None
        );
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

    /// A dark candle with the pool standing in passes, and the line names the
    /// degrade so the operator can tell the pool is carrying the mid.
    #[test]
    fn the_estimator_check_names_a_pool_only_composition() {
        let products = vec!["TGBP-USDC".to_string()];
        let now = THURSDAY_NOON;
        let rows = vec![FairPriceRow {
            anchor: "crypto_reference".to_string(),
            degrade: Some("pool_only".to_string()),
            health: "degraded".to_string(),
            basis: None,
            basis_age_secs: None,
            ..row("TGBP-USDC", now - 5, "degraded")
        }];
        let c = fair_verdict(&products, &rows, now, false);
        assert_eq!(c.verdict, Verdict::Pass, "{c}");
        assert!(
            c.evidence
                .contains("TGBP-USDC degraded (pool_only), anchor crypto_reference, no basis"),
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
            Verdict::Expected
        );
    }

    fn level(size_bps: u16, flip_spread_ppm: u32) -> Level {
        let mut l: Level = bytemuck::Zeroable::zeroed();
        l.size_bps = size_bps.into();
        l.flip_spread_ppm = flip_spread_ppm.into();
        l
    }

    #[test]
    fn only_posted_rungs_count_and_only_posted_flips() {
        let mut side = [level(0, 0); N_LEVELS];
        side[0] = level(2_500, 50);
        side[1] = level(2_500, 0);
        // Zero size with a flip spread: neither posted nor flipping.
        side[2] = level(0, 50);
        assert_eq!(rung_counts(&side), (2, 1));
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
        assert!(stale.evidence.ends_with("; maker stale"), "{stale}");
    }

    #[test]
    fn a_zero_quote_slot_reads_not_yet_not_never() {
        let unknown = market_verdict(
            "m".into(),
            Posture::Active,
            book(0, 0, 0, Liveness::Unknown),
            false,
        );
        assert_eq!(unknown.verdict, Verdict::Fail);
        assert!(
            unknown
                .evidence
                .ends_with("; no quote on chain yet (start the makers with M)"),
            "{unknown}"
        );
        let live = market_verdict(
            "m".into(),
            Posture::Active,
            book(4, 4, 0, Liveness::Live),
            false,
        );
        assert!(live.evidence.ends_with("; maker quoting"), "{live}");
    }

    #[test]
    fn a_failed_validator_skips_every_chain_line_once() {
        let validator = Check::new("validator", Verdict::Fail, "unreachable: refused");
        let checks = chain_skipped(validator.clone());
        let roster = market::roster(Cluster::Localnet);
        assert_eq!(checks.len(), roster.len() + 2);
        assert_eq!(checks[0], validator);
        assert_eq!(checks[1].surface, market_surface(roster[0]));
        assert_eq!(checks.last().unwrap().surface, QUOTE_SURFACE);
        for c in &checks[1..] {
            assert_eq!(c.verdict, Verdict::Skipped, "{c}");
            assert_eq!(
                c.evidence, "not checked — validator failed: unreachable: refused",
                "{c}"
            );
            assert!(c.to_string().starts_with("SKIPPED "), "{c}");
        }
    }

    #[test]
    fn the_header_says_the_check_reads_the_live_demo() {
        for needle in ["live demo", "second terminal", "Bootstrap complete", " M"] {
            assert!(HEADER.contains(needle), "{needle}: {HEADER}");
        }
    }

    #[test]
    fn the_summary_fails_only_on_a_fail_and_names_the_skips() {
        let pass = Check::new("frontend", Verdict::Pass, "");
        let expected = Check::new("mode", Verdict::Expected, "");
        assert_eq!(
            summary(&[pass.clone(), expected]),
            (true, "demo stack ready".into())
        );
        let down = chain_skipped(Check::new("validator", Verdict::Fail, "unreachable"));
        let skipped = down.len() - 1;
        let (passed, line) = summary(&[pass.clone()].into_iter().chain(down).collect::<Vec<_>>());
        assert!(!passed);
        assert!(
            line.starts_with(&format!("1 surface(s) failed; {skipped} skipped (market ")),
            "{line}"
        );
        assert!(line.contains(&format!("{QUOTE_SURFACE})")), "{line}");
        let only_skipped = Check::new("quote TGBP-USDC", Verdict::Skipped, "");
        let (passed, line) = summary(&[pass, only_skipped]);
        assert!(passed, "a skip alone is not a fail");
        assert!(line.contains("1 skipped (quote TGBP-USDC)"), "{line}");
    }

    #[test]
    fn keyless_excuses_an_empty_book_and_nothing_else() {
        let empty = market_verdict(
            "m".into(),
            Posture::Passive,
            book(0, 0, 0, Liveness::Unknown),
            true,
        );
        assert_eq!(empty.verdict, Verdict::Expected);
        assert!(empty.evidence.ends_with("halted, keyless"), "{empty}");
        let no_flip = market_verdict(
            "m".into(),
            Posture::Passive,
            book(4, 4, 0, Liveness::Unknown),
            true,
        );
        assert_eq!(no_flip.verdict, Verdict::Fail, "{no_flip}");
        let one_sided = market_verdict(
            "m".into(),
            Posture::Active,
            book(4, 0, 0, Liveness::Live),
            true,
        );
        assert_eq!(one_sided.verdict, Verdict::Fail, "{one_sided}");
    }

    #[test]
    fn an_empty_quote_is_the_keyless_halt_and_a_failure_otherwise() {
        let filled = quote_verdict(7_540_000_000, 2, 9, false);
        assert_eq!(filled.verdict, Verdict::Pass);
        assert!(
            filled.evidence.contains("7.5400 TGBP across 2 legs"),
            "{filled}"
        );
        assert_eq!(quote_verdict(0, 0, 9, true).verdict, Verdict::Expected);
        assert_eq!(quote_verdict(0, 0, 9, false).verdict, Verdict::Fail);
    }

    #[test]
    fn the_http_status_line_parses() {
        assert_eq!(parse_status("HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(parse_status("HTTP/1.0 503 Service Unavailable"), Some(503));
        assert_eq!(parse_status("SSH-2.0-OpenSSH"), None);
    }

    #[test]
    fn an_http_probe_reads_a_live_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 256];
            let _ = conn.read(&mut buf);
            conn.write_all(b"HTTP/1.1 307 Temporary Redirect\r\n\r\n")
                .unwrap();
        });
        let c = http_check("frontend", port, "/");
        server.join().unwrap();
        assert_eq!(c.verdict, Verdict::Pass, "{c}");
        assert!(c.evidence.ends_with("→ 307"), "{c}");
    }
}
