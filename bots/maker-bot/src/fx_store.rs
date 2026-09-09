//! The shared market-data store, read as an intraday FX source.
//!
//! Every other price source in this bot polls a venue directly. This one does
//! not, and the asymmetry is deliberate: the intraday FX venues (OANDA, Twelve
//! Data, Alpha Vantage) are **keyed and metered**, the market-data collectors
//! already poll them on a budget sized to the free tier, and a second consumer
//! on the same credential is a self-inflicted rate-limit landing on the price
//! anchor. So the collectors own the venue relationship and the maker reads
//! their rows.
//!
//! Two consequences worth stating, because they are the reason this module is
//! shaped the way it is.
//!
//! **The store is load-bearing, not a convenience.** Losing it is a halt, not
//! a degrade. The maker used to be free to keep quoting off whatever was still
//! live, and for a Postgres outage that would now mean quoting off a daily ECB
//! fix while the good data is simply absent — which is the risk the operator
//! declined. [`MAX_STORE_SILENCE`] and [`store_unavailable`] are that rule
//! made checkable; the tick loop routes a true answer to the halt path rather
//! than composing a price.
//!
//! **A row's age is a claim about the venue, not about the database.** See
//! [`store_reading_age`].
//!
//! # Source classes
//!
//! The venues split by publication cadence rather than by quality:
//!
//! | Venue           | `cex_prices.source` | Bucket | Offered as        |
//! | --------------- | ------------------- | ------ | ----------------- |
//! | OANDA           | `oanda`             | 1 min  | `Tape`, *trusted* |
//! | Twelve Data     | `twelvedata`        | 1 min  | `Tape`            |
//! | Alpha Vantage   | `alphavantage`      | daily  | `Reference`       |
//!
//! Alpha Vantage is pinned daily by its own free tier, so it is a reference
//! fix and not a tape — it is authoritative for the day it names and says
//! nothing about now. Offering it as a tape would let a fix hours old sit in
//! the fast median beside a live minute bar, which is exactly the pooling the
//! reference class exists to prevent.
//!
//! **OANDA is designated believable alone, and that is a ruling rather than a
//! default.** The degrade ladder has a quote-on-one-venue mode, and that mode
//! is only sound if the last venue standing is one the bot will trust without
//! corroboration — otherwise the ladder's bottom rung is unreachable and the
//! leg darks instead of degrading. OANDA is the roster's FX anchor by role
//! (deepest history, real tick volume, a per-candle `complete` flag), so it
//! carries that designation beside Pyth's. Twelve Data is untrusted by
//! default: it is a fine corroborating tape and nobody has argued it should
//! stand alone.
//!
//! Note "trusted" is not a [`dropset_fair_value::SourceClass`] — that enum is
//! `Tape` or `Reference` and nothing else. Trust is a separate attribute of a
//! candidate, which is why [`FxCandidateKind`] below is the product of the two
//! rather than an extension of either.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use dropset_feeds::{Batch, Source};
use sqlx::{PgPool, Row};

/// `cex_prices.source` for each intraday FX venue the collectors write.
pub const SOURCE_OANDA: &str = "oanda";
pub const SOURCE_TWELVEDATA: &str = "twelvedata";
pub const SOURCE_ALPHAVANTAGE: &str = "alphavantage";

/// Every venue this reader asks for, in the order candidates are offered.
///
/// Order is not priority — the engine resolves by consensus — it only decides
/// who survives a candidate set larger than the engine will hold.
pub const FX_STORE_SOURCES: [&str; 3] = [SOURCE_OANDA, SOURCE_TWELVEDATA, SOURCE_ALPHAVANTAGE];

/// How a store venue's reading is offered to the engine.
///
/// This is the product of two independent things — the engine's `SourceClass`
/// (`Tape` or `Reference`) and whether the candidate is trusted to stand alone
/// — because the engine models them separately: class picks the staleness
/// bound and whether the reading joins the fast median, trust decides whether
/// one source is a sufficient leg. Keeping the mapping here as data, rather
/// than as a chain of `if source == …` at the call site, is what lets the
/// designation be tested without a database or a tick loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxCandidateKind {
    /// A live tape that still needs corroboration.
    Tape,
    /// A live tape designated believable alone — the bottom rung of the
    /// degrade ladder rests on this existing.
    TrustedTape,
    /// A slow fix: authoritative for the moment it names, kept out of the fast
    /// median, and governed by the far looser reference staleness bound.
    Reference,
}

/// The ruled designation for a `cex_prices.source`, or `None` for a venue this
/// reader does not offer to the FX leg.
///
/// Returning `None` rather than defaulting to [`FxCandidateKind::Tape`] is
/// deliberate: an unrecognized source is a roster change nobody wired here,
/// and silently promoting it to a live tape would let a new collector start
/// pricing the book the moment it was switched on.
pub fn fx_candidate_kind(source: &str) -> Option<FxCandidateKind> {
    match source {
        SOURCE_OANDA => Some(FxCandidateKind::TrustedTape),
        SOURCE_TWELVEDATA => Some(FxCandidateKind::Tape),
        SOURCE_ALPHAVANTAGE => Some(FxCandidateKind::Reference),
        _ => None,
    }
}

/// The canonical store pair for a market's tracked currency — `AUD` →
/// `AUD-USD`.
///
/// The store normalizes all three venues' spellings onto this id, so it is the
/// one join key that works across sources. USD itself has no cross and returns
/// `None`.
pub fn fx_product_id(currency: &str) -> Option<String> {
    (currency != "USD").then(|| format!("{currency}-USD"))
}

/// How long the store may stay silent before the maker treats it as gone.
///
/// Sized off the slowest tape collector's own cadence rather than off the
/// engine's staleness bound: OANDA polls every 60s and writes minute buckets,
/// so a healthy read is never more than ~2 minutes behind. Five minutes is
/// several missed polls — long enough that a single slow round trip or a
/// collector restart does not halt a live book, short enough that a genuine
/// outage stops quoting well inside the 15-minute tape bound that would
/// otherwise let the leg fall through to a daily fix.
///
/// This is a **liveness** bound on the reader, deliberately distinct from the
/// engine's per-class staleness bound on a *reading*. A stale row and an
/// unreachable database are different failures: the first is one candidate
/// dropping out of a composition, the second is the composition no longer
/// resting on the data the operator chose to price off.
pub const MAX_STORE_SILENCE: Duration = Duration::from_secs(5 * 60);

/// One venue's newest closed bucket for one pair.
#[derive(Clone, Debug, PartialEq)]
pub struct FxStoreRow {
    /// `cex_prices.source` — the venue, e.g. `oanda`.
    pub source: String,
    /// The canonical pair, e.g. `AUD-USD`. Canonical on purpose: the venues
    /// spell it three different ways and the store normalizes them, so this
    /// joins cleanly across sources.
    pub product_id: String,
    /// Epoch second the bucket **closed** — the instant `close` describes.
    pub published_at: i64,
    /// The bucket's closing price, in USD per unit of the base currency.
    pub close: f64,
}

/// One poll's worth of rows — every series in one snapshot, so a consumer
/// caches them together and can tell "the store answered" from "this series
/// was missing".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FxStoreSnapshot {
    pub rows: Vec<FxStoreRow>,
}

/// The age to hand the engine for a store row.
///
/// This follows the same contract as the bot's other per-source readers:
///
/// ```text
/// age = max(publication_age, receipt_age)
/// ```
///
/// Both halves are load-bearing and neither is redundant.
///
/// **Publication age** is what makes the reading honest about the *venue*. The
/// row's `published_at` is the bucket close, so a stalled collector keeps
/// serving the same row and its publication age grows even though the maker
/// keeps reading it successfully. Ageing from the database write instead would
/// report a five-hour-old print as fresh the moment it was re-read.
///
/// **The receipt floor** is what makes it honest about the *reader*. A cached
/// snapshot whose poller has died has a stamp that stops moving, so flooring
/// on when this process last read the row is what ages the leg out anyway.
///
/// Taking the `max` of the two is also the forward-skew guard: a stamp can
/// only ever make a reading older, never younger, so a bogus future
/// `published_at` cannot pin the age at zero and manufacture a permanently
/// fresh candidate. That is why this is a `max` and not a choice between them.
pub fn store_reading_age(published_at: i64, now_unix: i64, receipt_age: Duration) -> Duration {
    let publication_secs = now_unix.saturating_sub(published_at).max(0);
    let publication_age = Duration::from_secs(publication_secs as u64);
    publication_age.max(receipt_age)
}

/// Whether the store has been silent long enough to count as gone.
///
/// `since_last_ok` is the time since the last **successful** read, not since
/// the last attempt: a read that keeps failing is silence, however busily it
/// is retried.
pub fn store_unavailable(since_last_ok: Duration) -> bool {
    since_last_ok > MAX_STORE_SILENCE
}

/// Polls the market-data store for the newest FX print per venue and pair.
///
/// Implemented as a [`Source`] so it rides the same spawn / backoff / health
/// machinery as the venue pollers rather than growing a parallel loop — from
/// the tick loop's side it is one more broadcast receiver to drain.
pub struct FxStoreSource {
    name: String,
    pool: PgPool,
    sources: Vec<String>,
    products: Vec<String>,
}

impl FxStoreSource {
    /// `products` are canonical pair ids (`AUD-USD`), `sources` the venue
    /// labels written to `cex_prices.source`.
    pub fn new(name: impl Into<String>, pool: PgPool, products: Vec<String>) -> Self {
        Self {
            name: name.into(),
            pool,
            sources: FX_STORE_SOURCES.iter().map(|s| s.to_string()).collect(),
            products,
        }
    }

    /// Read the newest closed bucket for every configured series.
    ///
    /// Runtime-typed, like every other query this bot runs: the SQL lives in
    /// `queries/` and is bound positionally, so the bot needs no
    /// `DATABASE_URL` at compile time and — more to the point — takes no
    /// dependency on the schema owner and asserts no schema version. It reads
    /// four columns of one table and tolerates the rest of the store moving
    /// underneath it.
    pub async fn latest(&self) -> Result<Vec<FxStoreRow>> {
        let rows = sqlx::query(include_str!("../queries/fx_store_latest.sql"))
            .bind(&self.sources)
            .bind(&self.products)
            .fetch_all(&self.pool)
            .await?;

        rows.iter()
            .map(|r| {
                Ok(FxStoreRow {
                    source: r.try_get("source")?,
                    product_id: r.try_get("product_id")?,
                    published_at: r.try_get("published_at")?,
                    close: r.try_get("close")?,
                })
            })
            .collect()
    }
}

#[async_trait]
impl Source for FxStoreSource {
    type Record = FxStoreSnapshot;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        let rows = self.latest().await?;
        // Always emit, even empty. An empty snapshot is a successful read that
        // found no rows, which is a different fact from a failed read, and the
        // consumer needs it to distinguish "the collectors are behind" from
        // "the store is gone" — the second halts, the first does not.
        Ok(Batch::new(vec![FxStoreSnapshot { rows }]).with_caught_up(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// The ordinary case: the venue print is older than the read, so it wins.
    #[test]
    fn publication_age_dominates_a_recent_read() {
        // Bucket closed 120s ago, read 1s ago.
        assert_eq!(store_reading_age(1_000, 1_120, secs(1)), secs(120));
    }

    /// A stalled poller sitting on a cached row still ages the leg out: the
    /// stamp stops moving, so the receipt floor is the only thing left that
    /// can grow.
    #[test]
    fn the_receipt_floor_ages_out_a_dead_poller() {
        // Print is 10s old by its stamp, but this process last read it 900s
        // ago — the floor, not the stamp, is the honest age.
        assert_eq!(store_reading_age(1_000, 1_010, secs(900)), secs(900));
    }

    /// A bogus future stamp must not pin the age at zero. Taking the `max`
    /// means a stamp can only ever make a reading older.
    #[test]
    fn a_future_stamp_cannot_manufacture_freshness() {
        // `published_at` is 500s in the future; the read was 300s ago.
        let age = store_reading_age(2_000, 1_500, secs(300));
        assert_eq!(age, secs(300), "the receipt floor must survive skew");
    }

    /// Saturating rather than panicking on an absurd stamp.
    #[test]
    fn an_absurd_stamp_saturates() {
        assert_eq!(store_reading_age(i64::MAX, 0, secs(7)), secs(7));
        assert!(store_reading_age(i64::MIN, 0, secs(7)) >= secs(7));
    }

    /// The liveness bound is on *successful* reads and is strictly greater
    /// than the threshold, so a read exactly at the bound still counts.
    #[test]
    fn store_silence_is_bounded() {
        assert!(!store_unavailable(secs(0)));
        assert!(!store_unavailable(MAX_STORE_SILENCE));
        assert!(store_unavailable(MAX_STORE_SILENCE + secs(1)));
    }

    /// The designations are a ruling, not a default, so they are pinned here
    /// rather than left to whatever the call site happens to pass.
    #[test]
    fn the_ruled_designations_hold() {
        assert_eq!(
            fx_candidate_kind(SOURCE_OANDA),
            Some(FxCandidateKind::TrustedTape),
            "the degrade ladder's one-venue rung needs a believable-alone tape"
        );
        assert_eq!(
            fx_candidate_kind(SOURCE_TWELVEDATA),
            Some(FxCandidateKind::Tape)
        );
        assert_eq!(
            fx_candidate_kind(SOURCE_ALPHAVANTAGE),
            Some(FxCandidateKind::Reference),
            "a daily series is a fix, never a tape"
        );
    }

    /// A source nobody wired must not start pricing the book by default.
    #[test]
    fn an_unknown_source_is_not_offered() {
        assert_eq!(fx_candidate_kind("kraken"), None);
        assert_eq!(fx_candidate_kind(""), None);
    }

    /// Every venue the reader asks the store for must have a designation, or
    /// its rows would be fetched and then silently dropped.
    #[test]
    fn every_requested_source_is_designated() {
        for source in FX_STORE_SOURCES {
            assert!(
                fx_candidate_kind(source).is_some(),
                "{source} is queried but has no designation"
            );
        }
    }

    #[test]
    fn a_currency_maps_onto_its_canonical_pair() {
        assert_eq!(fx_product_id("AUD").as_deref(), Some("AUD-USD"));
        assert_eq!(fx_product_id("CAD").as_deref(), Some("CAD-USD"));
        assert_eq!(fx_product_id("USD"), None, "USD has no cross");
    }

    /// The SQL beside the `.bind()` chain has to agree with it, and it is
    /// runtime-typed, so nothing else checks this until the query runs. Two
    /// binds, and every column the row decoder reads by name must be a name
    /// the statement actually projects — an aliased column silently renamed
    /// fails at `try_get`, in production, on the price path.
    #[test]
    fn the_query_matches_its_binds_and_its_decoder() {
        let sql = include_str!("../queries/fx_store_latest.sql");

        // `$10` must not read as `$1`, so take the digits, not one char.
        let highest = sql
            .match_indices('$')
            .filter_map(|(i, _)| {
                let digits: String = sql[i + 1..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.parse::<usize>().ok()
            })
            .max()
            .expect("the query binds at least one parameter");
        assert_eq!(highest, 2, "placeholder count drifted from the binds");

        for column in ["source", "product_id", "published_at", "close"] {
            assert!(
                sql.contains(column),
                "the decoder reads `{column}` but the statement does not project it"
            );
        }
    }

    /// The liveness bound must fire well before the engine's 15-minute tape
    /// bound, or a Postgres outage would silently become a fall-through to the
    /// daily reference tier — the exact behavior the halt rule forbids.
    #[test]
    fn silence_bound_precedes_the_tape_staleness_bound() {
        assert!(
            MAX_STORE_SILENCE < secs(15 * 60),
            "the maker must halt on a dead store before its tape candidates \
             age out into the reference tier"
        );
    }
}
