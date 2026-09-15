//! The shared market-data store, read as a **spot-tick** source — the
//! `spot_ticks` counterpart to [`crate::fx_store`].
//!
//! # Why this exists
//!
//! `USDC-USD` is written by exactly one collector, `market-data-kraken`, and it
//! writes `spot_ticks`. **Nothing writes that series to `cex_prices`**, which is
//! the only table [`crate::fx_store`] reads — so the fair-value estimator's
//! USDC/USD peg leg was unreachable through the reader it was meant to use.
//!
//! Which writer lands where, since it is not guessable from a venue's name (each
//! collector's own `SOURCE` const doc is the authority; this table is a
//! convenience copy and the thing in this module most likely to go quietly stale):
//!
//! | Table        | Collectors                                            |
//! | ------------ | ----------------------------------------------------- |
//! | `cex_prices` | coinbase (candles), oanda, twelvedata, alphavantage    |
//! | `spot_ticks` | coinbase-ticker, kraken, erapi, frankfurter, pyth      |
//!
//! Note `coinbase` appears on both rows: the candle collector and the ticker
//! share one source label and write different tables, so a `(source, product_id)`
//! pair only identifies a series **together with the table**.
//!
//! # A separate reader rather than a widened one
//!
//! The two tables answer the same question about different row shapes, and
//! neither shape can be expressed as the other without lying: a candle's
//! publication instant is **derived** (`bucket_start + granularity_secs`), a
//! tick's is **recorded** (`observed_at`). Migration 0004 declined to synthesize
//! a bucket from a tick when it created this table, and that reasoning is now
//! immutable. `queries/spot_ticks_latest.sql` carries the full argument,
//! including why a `UNION` would have to mislabel one side.
//!
//! What **is** shared is the age convention, not the SQL: both readers hand their
//! rows to [`crate::fx_store::store_reading`], so publication-versus-receipt
//! ageing and the forward-skew refusal have exactly one home.
//!
//! That home is **provisional**. `store_reading` lives in the candle module and
//! its parameters are spelled in candle vocabulary (`close`, `published_at`),
//! which a tick caller has to translate. With two readers this is the right
//! trade — one convention beats a tidy name — but a third reader would be the
//! point to lift the ageing pair into a module of its own.
//!
//! # What this reader does NOT judge
//!
//! It refuses a **forward-stamped** row and ages everything else. It does not
//! judge the *value*: a non-finite, zero or negative `price` decodes and is
//! offered. That is not an omission, and the component that refuses it is
//! nameable — [`dropset_fair_value::Reading::valid`], reached through `Reading::fresh`,
//! which `Candidates::resolve` applies to every candidate before it can reach a
//! comparison. See [`SpotTickRow::reading`], which states the boundary precisely.
//!
//! The **confidence half-width is deliberately unread** — the projection carries
//! `price` only. `queries/spot_ticks_latest.sql` gives the reasoning; a unit test
//! below asserts the decision so it cannot lapse silently.

use std::time::Duration;

use anyhow::Result;
use dropset_fair_value::Reading;
use sqlx::{PgPool, Row};

use crate::fx_store::store_reading;

/// `spot_ticks.source` for the venue that publishes peg truth.
///
/// A real market print of `USDC/USD` rather than an issuer redemption rate —
/// Circle publishes no keyless endpoint, so this stands in for peg truth.
///
/// **This is a join by string against `market-data/src/bin/kraken.rs`'s own
/// private `SOURCE`, and nothing pins the two equal.** If that collector's label
/// changed, this reader would ask for a series nobody writes and the peg leg
/// would empty silently. Deduplicating it (one `pub` definition the bin imports)
/// is the real fix and is deliberately not attempted here — it would change a
/// collector this PR does not otherwise touch.
pub const SOURCE_KRAKEN: &str = "kraken";

/// The canonical peg series every market's common-mode guard reads.
///
/// **Portfolio-wide, not per-market.** A USDC/USD deviation is one event for the
/// whole book (`docs/market-making.md` §1 fm1), so this is one series rather than
/// one per pair, and a consumer offers the same resolved reading to every market.
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
    /// when the row's **stamp** makes it unusable.
    ///
    /// Delegates to [`store_reading`] rather than ageing the row itself, which is
    /// the whole of this reader's freshness policy: one convention, shared with
    /// the candle reader, so a forward-stamped tick is refused on exactly the
    /// same terms as a forward-stamped bucket.
    ///
    /// **`None` means the stamp was refused — it does not mean the value was
    /// judged.** A non-finite, zero or negative `price` returns `Some`, and that
    /// is deliberate rather than an oversight: the pricing crate already refuses
    /// such a value at the point it would matter.
    /// [`dropset_fair_value::Reading::valid`] is `is_finite() && > 0.0`, and
    /// `Reading::fresh` — which `Candidates::resolve` applies to every candidate
    /// — is `young() && valid()`. So an unusable value never reaches a
    /// comparison, and `Candidates::any_invalid` reports it distinctly so the
    /// operator sees a live-but-garbage feed rather than an absent one.
    ///
    /// Keeping the check there rather than here is what lets that distinction
    /// exist: a row this reader dropped would be indistinguishable from a venue
    /// that published nothing. `a_non_finite_price_is_passed_on_for_the_engine_to_refuse`
    /// pins both halves of this contract.
    pub fn reading(&self, now_unix: i64, receipt_age: Duration) -> Option<Reading> {
        store_reading(self.price, self.observed_at, now_unix, receipt_age)
    }
}

/// Polls the market-data store for the newest spot print per venue and pair.
///
/// A **publisher** should call [`Self::latest`] directly rather than driving this
/// through the feeds runner: that runner sleeps its backoff and continues on any
/// source error, forever — right for a collector, where a missed poll is a gap in
/// a series, and wrong for something that has to decide whether a failure is
/// worth retrying at all.
///
/// It deliberately does **not** implement [`dropset_feeds::Source`], unlike its
/// candle sibling. The sibling's impl has a live consumer that genuinely rides
/// the framework's spawn / backoff / health machinery; this reader's first
/// consumer takes the direct path above, so a trait impl here would be carried
/// by symmetry alone — untested, and shaped by a guess about a consumer that
/// does not exist. The PR that first needs a framework-driven tick can add it
/// with a test that drains it.
///
/// **Deriving `Debug` on this type would leak connection details.** `sqlx`'s
/// `PgConnectOptions` has a derived `Debug` carrying host, user and database, so
/// the absence of a derive here is deliberate rather than an omission.
pub struct SpotTickSource {
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
    /// legs. [`Self::peg`] does bake one in, and is safe to because this
    /// unopinionated constructor stays public beside it.
    ///
    /// Two properties of the resulting query a caller has to know, because
    /// neither is what the parameter names suggest:
    ///
    /// - **The two lists are combined as a CROSS PRODUCT, not zipped into
    ///   pairs.** Asking for sources `[a, b]` and products `[x, y]` returns every
    ///   one of those four series that exists — not `a/x` and `b/y`. For a
    ///   one-source, one-product roster like [`Self::peg`] the distinction is
    ///   empty; for a multi-leg roster it is not, and it is not hypothetical:
    ///   `kraken` writes `EURC-USDC` as well as `USDC-USD`. A multi-leg caller
    ///   must therefore key the returned rows by `(source, product_id)` itself,
    ///   or use one reader per leg.
    /// - **An empty list is a silent, permanent empty result.** `= ANY('{}')`
    ///   matches nothing, so an empty `sources` or `products` yields `Ok(vec![])`
    ///   on every poll — indistinguishable, to a consumer, from collectors that
    ///   have written nothing yet. Nothing here rejects it, because a roster is
    ///   the consumer's policy; a consumer assembling one per market should treat
    ///   an empty leg roster as a misconfiguration rather than as no data.
    pub fn new(pool: PgPool, sources: Vec<String>, products: Vec<String>) -> Self {
        Self {
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
    /// constants. Baking a roster in is safe here precisely because
    /// [`Self::new`] remains the unopinionated path.
    pub fn peg(pool: PgPool) -> Self {
        Self::new(
            pool,
            vec![SOURCE_KRAKEN.to_string()],
            vec![USDC_USD_PRODUCT.to_string()],
        )
    }

    /// The venue labels this reader asks the store for.
    ///
    /// Exposed so a roster constructor can be tested on what it actually
    /// composes rather than on the constants it was built from — see
    /// `the_peg_roster_asks_for_one_venue_and_one_series`.
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// The canonical pair ids this reader asks the store for.
    pub fn products(&self) -> &[String] {
        &self.products
    }

    /// Read the newest print for every configured series.
    ///
    /// Runtime-typed, like every other query in this crate: the SQL lives in
    /// `queries/` and is bound positionally, so **this reader asserts no schema
    /// version**. It reads four columns of one table and tolerates the rest of
    /// the store moving underneath it.
    ///
    /// An empty result is a successful read, not an error — a consumer's
    /// store-silence guard keys off the difference between "the collectors are
    /// behind" and "the store is gone".
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The statement with its `--` header stripped.
    ///
    /// Every assertion about the SQL has to run against this rather than the raw
    /// file: the header is long-form prose that names the projected columns and
    /// discusses a placeholder the statement does not bind, so a raw-text scan
    /// measures the commentary.
    fn statement_body() -> String {
        include_str!("../queries/spot_ticks_latest.sql")
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n")
    }

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
        // Overshoot inside the shared 120s forward-skew bound is still
        // tolerated. 60s ahead is half that bound, so this exercises the
        // tolerance rather than its edge.
        let ok = row(USDC_USD_PRODUCT, 1_560, 1.0);
        assert!(ok.reading(1_500, Duration::from_secs(1)).is_some());
    }

    /// A non-finite price is passed on, and the pricing crate is what refuses it.
    ///
    /// **Both halves are asserted, because either one alone would be
    /// misleading.** That this reader returns `Some` is only defensible if
    /// something downstream says no — so the test also exercises the predicate
    /// that does, rather than asserting a boundary and trusting prose about the
    /// other side of it. If `Reading::valid` ever stopped covering these values,
    /// this fails here rather than surfacing as a guard that silently never
    /// fires.
    ///
    /// `NaN` is the case that matters: it defeats *comparison* rather than
    /// arithmetic, so a deviation guard written as `value > tol` would not fire
    /// on it. Zero and negative are included because `valid()` covers them too.
    #[test]
    fn a_non_finite_price_is_passed_on_for_the_engine_to_refuse() {
        let bound = Duration::from_secs(300);
        for price in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
            let offered = row(USDC_USD_PRODUCT, 1_000, price)
                .reading(1_060, Duration::from_secs(1))
                .expect("the stamp is honest, so the row is offered");
            assert!(
                !offered.valid(),
                "{price} must be refused by the engine's own predicate"
            );
            assert!(
                !offered.fresh(bound),
                "{price} must not survive the freshness filter `resolve` applies"
            );
        }

        // The control: an honest value passes both, so the assertions above are
        // not holding for some unrelated reason.
        let good = row(USDC_USD_PRODUCT, 1_000, 0.9999)
            .reading(1_060, Duration::from_secs(1))
            .expect("offered");
        assert!(good.valid() && good.fresh(bound));
    }

    /// The peg constructor composes the roster it claims to.
    ///
    /// **Asserted on what `peg()` actually builds, not on the two constants.** An
    /// earlier version of this test compared `SOURCE_KRAKEN` and
    /// `USDC_USD_PRODUCT` against their own literals, which is vacuous twice
    /// over: it never called `peg()`, so rewriting that constructor to ask for a
    /// different venue left the test green, and it restated the definitions
    /// rather than pinning anything.
    ///
    /// Note what this still cannot pin: that `"kraken"` is the label the
    /// collector writes. Nothing does — see [`SOURCE_KRAKEN`].
    #[tokio::test]
    async fn the_peg_roster_asks_for_one_venue_and_one_series() {
        // No server is needed to inspect the composed roster — a lazy pool never
        // connects. It does need a Tokio context to construct, which is the only
        // reason this is an async test.
        let pool = PgPool::connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .expect("a lazy pool needs no server");
        let peg = SpotTickSource::peg(pool);
        assert_eq!(peg.sources(), [SOURCE_KRAKEN]);
        assert_eq!(peg.products(), [USDC_USD_PRODUCT]);
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
        // Scan the STATEMENT, not the file. The header is prose and discusses
        // both column names and a hypothetical `$3` bound, so scanning the whole
        // file makes every assertion below hostage to the commentary — which is
        // not hypothetical: the placeholder assertion caught exactly that.
        let sql = statement_body();

        // Every placeholder `$1..=$n` must appear, so a duplicated or skipped
        // index fails here. Asserting only the maximum would pass
        // `ANY($2) AND ANY($2)`, which returns no rows in production.
        let mut seen: Vec<usize> = sql
            .match_indices('$')
            .filter_map(|(i, _)| {
                let digits: String = sql[i + 1..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.parse::<usize>().ok()
            })
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, vec![1, 2], "placeholder set drifted from the binds");

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
        // than trusting the prose above to keep it. Checked against the whole
        // SELECT list rather than the resolved output names, because an alias
        // (`confidence AS half_width`) would hide the column from `projected`
        // while still wiring it.
        assert!(
            !select_list.contains("confidence"),
            "the confidence half-width is deliberately unread under the MVP posture"
        );
    }

    /// This reader must not read `cex_prices`, which is the whole reason it
    /// exists. Asserted against the statement because the two files are
    /// otherwise near-identical and a copy-paste would compile and pass every
    /// other test here.
    #[test]
    fn the_statement_reads_the_tick_table() {
        let body = statement_body();
        assert!(body.contains("FROM spot_ticks"), "{body}");
        assert!(!body.contains("cex_prices"), "{body}");
    }
}
