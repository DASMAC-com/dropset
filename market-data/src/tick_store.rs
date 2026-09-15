//! The shared market-data store, read as a **spot-tick** source — the
//! `spot_ticks` counterpart to [`crate::fx_store`].
//!
//! # Why this exists at all
//!
//! The candle reader beside it claims, in its own SQL header, that "the
//! fair-value estimator reads the crypto reference and the USDC/USD peg the same
//! way". That is true of the crypto reference and **false of the peg**, and the
//! difference is a deployment fact rather than a query one: `USDC-USD` is
//! written by exactly one collector — `market-data-kraken` — and that collector
//! writes `spot_ticks`. Nothing writes the series to `cex_prices` at all.
//!
//! The claim reads as settled because the candle reader's own test inserts a
//! `kraken`/`USDC-USD` row into `cex_prices` and then reads it back. That
//! proves the statement is not source-constrained; it proves nothing about any
//! collector putting a row there. So the estimator's peg leg needed a reader for
//! the table the row is actually in.
//!
//! Which writer lands where, since the split is not guessable from a venue's
//! name:
//!
//! | Table        | Collectors                                            |
//! | ------------ | ----------------------------------------------------- |
//! | `cex_prices` | coinbase (candles), oanda, twelvedata, alphavantage    |
//! | `spot_ticks` | coinbase-ticker, kraken, erapi, frankfurter, pyth      |
//!
//! # What is shared with the candle reader, and what is not
//!
//! **The age convention is shared; the row shape is not.** Both readers hand
//! their rows to [`crate::fx_store::store_reading`], so the
//! publication-versus-receipt `max`, the forward-skew refusal, and
//! [`crate::fx_store::MAX_PUBLICATION_SKEW`] are stated once and cannot drift
//! into a second convention. What is *not* shared is the row type: a candle's
//! publication instant is derived (`bucket_start + granularity_secs`) and a
//! tick's is recorded (`observed_at`), so reusing one struct would mean calling
//! a tick's price a `close`. The SQL header says more about why a `UNION` is
//! the wrong shape.
//!
//! **The confidence half-width is deliberately unread.** Pyth is the only venue
//! that publishes one, it is parked dark under the MVP posture, and the
//! fresh-but-uncertain regime it feeds is out of this scope — so this reader
//! projects `price` only. That is a decision, not an omission: a half-width read
//! into a leg nothing consumes would invite treating a NULL as a zero, which
//! reads as perfect certainty.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use dropset_fair_value::Reading;
use dropset_feeds::{Batch, Source};
use sqlx::{PgPool, Row};

use crate::fx_store::store_reading;

/// `spot_ticks.source` for the venue that publishes peg truth.
///
/// A real market print of `USDC/USD` rather than an issuer redemption rate —
/// Circle publishes no keyless endpoint, so this stands in for peg truth. It
/// matches `market-data/src/bin/kraken.rs`'s own `SOURCE` literally: a join by
/// string, so the two sides have to move together.
pub const SOURCE_KRAKEN: &str = "kraken";

/// The canonical peg series every market's common-mode guard reads.
///
/// **Portfolio-wide, not per-market.** A USDC/USD deviation is one event for the
/// whole book (§1 fm1), so this is one series rather than one per pair, and the
/// estimator offers the same resolved reading to every market it composes.
pub const USDC_USD_PRODUCT: &str = "USDC-USD";

/// One venue's newest spot print for one pair.
///
/// Distinct from [`crate::fx_store::FxStoreRow`] on purpose — see the module
/// docs. `observed_at` is recorded rather than derived, and `price` is one
/// observation rather than a bucket's closing aggregate.
#[derive(Clone, Debug, PartialEq)]
pub struct SpotTickRow {
    /// `spot_ticks.source` — the venue, e.g. `kraken`.
    pub source: String,
    /// The canonical pair, e.g. `USDC-USD`.
    pub product_id: String,
    /// Epoch second the reading is attributed to: the venue's own publish time
    /// where it publishes one, else the collector's poll second (0004).
    pub observed_at: i64,
    /// The observed price, in quote units per unit of the base.
    pub price: f64,
}

impl SpotTickRow {
    /// The [`Reading`] this row should be offered to the engine as, or `None`
    /// if it must not be offered at all.
    ///
    /// Delegates to [`store_reading`] rather than ageing the row itself, which
    /// is the whole of this reader's freshness policy: one convention, shared
    /// with the candle reader, so a forward-stamped tick is refused on exactly
    /// the same terms as a forward-stamped bucket.
    pub fn reading(&self, now_unix: i64, receipt_age: Duration) -> Option<Reading> {
        store_reading(self.price, self.observed_at, now_unix, receipt_age)
    }
}

/// One poll's worth of ticks — every series in one snapshot, so a consumer can
/// tell "the store answered" from "this series was missing".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpotTickSnapshot {
    pub rows: Vec<SpotTickRow>,
}

/// Polls the market-data store for the newest spot print per venue and pair.
///
/// Implemented as a [`Source`] for symmetry with
/// [`crate::fx_store::FxStoreSource`], so a consumer that wants it on the
/// framework's spawn / backoff machinery can have it there. The estimator does
/// **not** use it that way and calls [`Self::latest`] directly — see
/// [`crate::estimator`] for why the runner's retry policy is the wrong one for a
/// publisher.
pub struct SpotTickSource {
    name: String,
    pool: PgPool,
    sources: Vec<String>,
    products: Vec<String>,
}

impl SpotTickSource {
    /// `sources` are the venue labels written to `spot_ticks.source`;
    /// `products` are canonical pair ids (`USDC-USD`).
    ///
    /// **Stated in signature order deliberately**, for the reason the candle
    /// reader's constructor gives: they are adjacent `Vec<String>` parameters, so
    /// a transposed call compiles and returns no error — merely no rows.
    ///
    /// Both lists are the caller's. This reader holds no roster of its own,
    /// because a roster is a policy its consumer owns and baking one in is
    /// exactly what made the candle reader unable to reach a second consumer's
    /// legs.
    pub fn new(
        name: impl Into<String>,
        pool: PgPool,
        sources: Vec<String>,
        products: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            pool,
            sources,
            products,
        }
    }

    /// The peg roster's spelling of [`Self::new`] — one venue, one series.
    ///
    /// Sugar over passing the two constants by hand, kept for the same reason
    /// [`crate::fx_store::FxStoreSource::fx`] exists: every current caller wants
    /// exactly this, and one assembling the pair itself could drift from the
    /// constants the collector's own `SOURCE` is pinned against.
    pub fn peg(name: impl Into<String>, pool: PgPool) -> Self {
        Self::new(
            name,
            pool,
            vec![SOURCE_KRAKEN.to_string()],
            vec![USDC_USD_PRODUCT.to_string()],
        )
    }

    /// Read the newest print for every configured series.
    ///
    /// Runtime-typed, like every other query in this crate: the SQL lives in
    /// `queries/` and is bound positionally, so **this reader asserts no schema
    /// version**. It reads four columns of one table and tolerates the rest of
    /// the store moving underneath it.
    pub async fn latest(&self) -> Result<Vec<SpotTickRow>> {
        let rows = sqlx::query(include_str!("../queries/spot_ticks_latest.sql"))
            .bind(&self.sources)
            .bind(&self.products)
            .fetch_all(&self.pool)
            .await?;

        rows.iter()
            .map(|r| {
                Ok(SpotTickRow {
                    source: r.try_get("source")?,
                    product_id: r.try_get("product_id")?,
                    observed_at: r.try_get("observed_at")?,
                    price: r.try_get("price")?,
                })
            })
            .collect()
    }
}

#[async_trait]
impl Source for SpotTickSource {
    type Record = SpotTickSnapshot;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        let rows = self.latest().await?;
        // Always emit, even empty — an empty snapshot is a successful read that
        // found no rows, which is a different fact from a failed read.
        Ok(Batch::new(vec![SpotTickSnapshot { rows }]).with_caught_up(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(product_id: &str, observed_at: i64, price: f64) -> SpotTickRow {
        SpotTickRow {
            source: SOURCE_KRAKEN.to_string(),
            product_id: product_id.to_string(),
            observed_at,
            price,
        }
    }

    /// A row ages from its own stamp, floored on the read — the candle reader's
    /// convention, reached through the shared function rather than restated.
    #[test]
    fn a_tick_ages_from_its_stamp_floored_on_the_read() {
        let r = row(USDC_USD_PRODUCT, 1_000, 1.0);
        let reading = r
            .reading(1_060, Duration::from_secs(1))
            .expect("an honest tick is offered");
        assert_eq!(reading.age, Duration::from_secs(60));
        assert_eq!(reading.value, 1.0);

        // The receipt floor ages out a dead poller sitting on a cached row.
        let stalled = r
            .reading(1_010, Duration::from_secs(900))
            .expect("still offered");
        assert_eq!(stalled.age, Duration::from_secs(900));
    }

    /// A forward-stamped tick is refused outright rather than aged.
    ///
    /// This is the property that would silently differ if this reader had grown
    /// its own ageing: the receipt floor resets on every poll, so a skewed row
    /// aged rather than refused reads as permanently fresh and would keep the
    /// peg leg satisfied off a nonsense stamp.
    #[test]
    fn a_future_stamped_tick_is_refused() {
        let r = row(USDC_USD_PRODUCT, 2_000, 1.0);
        assert!(r.reading(1_000, Duration::from_secs(1)).is_none());
        // One bucket width of overshoot is still tolerated, per the shared bound.
        let ok = row(USDC_USD_PRODUCT, 1_560, 1.0);
        assert!(ok.reading(1_500, Duration::from_secs(1)).is_some());
    }

    /// The peg constructor asks for exactly the series the peg collector writes.
    ///
    /// Pinned because both halves are join-by-string against
    /// `market-data/src/bin/kraken.rs`, and a drift on either side returns no
    /// error — just an empty peg leg, which composes as "the guard cannot fire"
    /// rather than as a fault.
    #[test]
    fn the_peg_roster_is_one_venue_and_one_series() {
        assert_eq!(SOURCE_KRAKEN, "kraken");
        assert_eq!(USDC_USD_PRODUCT, "USDC-USD");
    }

    /// The SQL beside the `.bind()` chain has to agree with it, and it is
    /// runtime-typed, so nothing else checks this until the query runs.
    ///
    /// Compared against the statement's **projected output names** rather than
    /// against its raw text, for the reason the candle reader's twin test
    /// records: this file's header discusses `confidence` and `price` in prose,
    /// so a substring sweep would hold whatever the statement did.
    #[test]
    fn the_query_matches_its_binds_and_its_decoder() {
        let sql = include_str!("../queries/spot_ticks_latest.sql");

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

        let select_list = sql
            .split("SELECT DISTINCT ON (source, product_id)")
            .nth(1)
            .and_then(|rest| rest.split("FROM").next())
            .expect("the statement has a SELECT list");
        let projected: Vec<String> = select_list
            .split(',')
            .filter_map(|item| {
                let item = item.trim();
                if item.is_empty() {
                    return None;
                }
                let name = match item.rsplit_once(" AS ") {
                    Some((_, alias)) => alias,
                    None => item.split_whitespace().next_back()?,
                };
                Some(name.trim().to_string())
            })
            .collect();

        for column in ["source", "product_id", "observed_at", "price"] {
            assert!(
                projected.iter().any(|p| p == column),
                "the decoder reads `{column}` but the statement projects {projected:?}"
            );
        }

        // The half-width is unread by decision, so assert the decision rather
        // than trusting the prose above to keep it. A `confidence` appearing in
        // the projection is the wiring this module declined.
        assert!(
            !projected.iter().any(|p| p == "confidence"),
            "the confidence half-width is deliberately unread under the MVP posture"
        );
    }

    /// This reader must not read `cex_prices`, which is the whole reason it
    /// exists. Asserted against the statement because the two files are
    /// otherwise near-identical and a copy-paste would compile and pass every
    /// other test here.
    #[test]
    fn the_statement_reads_the_tick_table() {
        let sql = include_str!("../queries/spot_ticks_latest.sql");
        let body = sql
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains("FROM spot_ticks"), "{body}");
        assert!(!body.contains("cex_prices"), "{body}");
    }
}
