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
//! immutable. `queries/tick_store_latest.sql` carries the full argument,
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
//! # What this reader judges, and what it does not
//!
//! **Be precise about who refuses what, because a consumer chooses.**
//! [`TickStoreReader::latest`] returns **every** row it read, forward-stamped ones
//! included. The skew refusal lives in [`TickStoreRow::reading`], and the row's
//! fields are public — so a consumer that reads `price` directly opts out of the
//! whole convention. The first consumer is told to call `latest()` directly (see
//! that method), which makes this worth stating rather than implying: route
//! through `reading()` or you have no freshness policy at all.
//!
//! Neither method judges the **value**. A non-finite, zero or negative `price`
//! decodes and is offered; [`TickStoreRow::reading`] states which component
//! refuses it and what that costs.
//!
//! The **confidence half-width is deliberately unread** — the projection carries
//! `price` only. `queries/tick_store_latest.sql` gives the reasoning; a unit test
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
/// whole book (`docs/market-making.md` §1, "Regimes and failure modes"), so this
/// is one series rather than one per pair, and a consumer offers the same
/// resolved reading to every market.
pub const USDC_USD_PRODUCT: &str = "USDC-USD";

/// One venue's newest spot print for one pair.
///
/// Distinct from [`crate::fx_store::FxStoreRow`] on purpose — see the module
/// docs. `observed_at` is recorded rather than derived, and `price` is one
/// observation rather than a bucket's closing aggregate.
///
/// **`PartialEq` is non-reflexive on a row carrying a non-finite `price`**, which
/// this module documents as reachable. Derived anyway, mirroring the candle
/// sibling: it is what lets a test compare a decoded row against an expected one,
/// and no caller compares whole rows on the price path. Compare fields rather
/// than rows if a `NaN` could be in play.
#[derive(Clone, Debug, PartialEq)]
pub struct TickStoreRow {
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

impl TickStoreRow {
    /// The [`Reading`] this row should be offered to the engine as, or `None`
    /// when the row's **stamp** makes it unusable.
    ///
    /// Delegates to [`store_reading`] rather than ageing the row itself, which is
    /// the whole of this reader's freshness policy: one convention, shared with
    /// the candle reader, so a forward-stamped tick is refused on exactly the
    /// same terms as a forward-stamped bucket.
    ///
    /// **`None` means the stamp was refused — it does not mean the value was
    /// judged.** A non-finite, zero or negative `price` returns `Some`, and the
    /// component that refuses it is [`dropset_fair_value::Reading::valid`]
    /// (`is_finite() && > 0.0`), reached through `Reading::fresh`, which
    /// `Candidates::resolve` applies to every candidate. So along that path an
    /// unusable value cannot reach a comparison.
    ///
    /// **Two bounds on that reassurance, both worth knowing before relying on
    /// it.** It holds only for a consumer that actually routes through
    /// `Candidates::resolve` — nothing here obliges one to. And the engine's
    /// `Candidates::any_invalid`, which distinguishes a live-but-garbage feed
    /// from an absent one, has **no peg-leg call site** today: it is called for
    /// the FX anchor only. So a garbage USDC/USD print is currently filtered and
    /// reads as an **absent** peg leg, not as a faulted one. Giving the peg leg
    /// that distinction is a fair-value-crate change, not a reader change.
    ///
    /// `a_price_the_engine_refuses_is_still_passed_on` pins both halves of the
    /// contract this method does own.
    pub fn reading(&self, now_unix: i64, receipt_age: Duration) -> Option<Reading> {
        store_reading(self.price, self.observed_at, now_unix, receipt_age)
    }
}

/// Reads the market-data store for the newest spot print per venue and pair.
///
/// **Named a reader rather than a source deliberately**: it does not implement
/// [`dropset_feeds::Source`], unlike its candle sibling, so calling it a `Source`
/// would assert a trait relationship it does not have.
///
/// A **publisher** should call [`Self::latest`] directly rather than driving a
/// reader through the feeds runner: that runner sleeps its backoff and continues
/// on any source error, forever — right for a collector, where a missed poll is a
/// gap in a series, and wrong for something that has to decide whether a failure
/// is worth retrying at all.
///
/// The sibling's trait impl has a live consumer that genuinely rides the
/// framework's spawn / backoff / health machinery; this reader's first consumer
/// takes the direct path above, so an impl here would be carried by symmetry
/// alone — untested, and shaped by a guess about a consumer that does not exist.
/// The PR that first needs a framework-driven tick can add it with a test that
/// drains it.
///
/// No `Debug` derive, and **not** because it would leak a credential: `PgPool`'s
/// own `Debug` prints pool sizing (`size`, `num_idle`, `is_closed`, and its
/// `PoolOptions`) and never reaches the connect options, so a derive would expose
/// nothing but the two rosters. It is simply unused — nothing formats this type.
pub struct TickStoreReader {
    pool: PgPool,
    sources: Vec<String>,
    products: Vec<String>,
}

impl TickStoreReader {
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
    /// **Returns every row it read**, including a forward-stamped one — see the
    /// module docs on who refuses what. An empty result is a successful read,
    /// not an error: a consumer's store-silence guard keys off the difference
    /// between "the collectors are behind" and "the store is gone".
    pub async fn latest(&self) -> Result<Vec<TickStoreRow>> {
        let rows = sqlx::query(include_str!("../queries/tick_store_latest.sql"))
            .bind(&self.sources)
            .bind(&self.products)
            .fetch_all(&self.pool)
            .await?;

        rows.iter()
            .map(|r| {
                Ok(TickStoreRow {
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
    /// **Every assertion about the SQL runs against this, not the raw file.** The
    /// header is long-form prose that names the projected columns and discusses a
    /// placeholder the statement does not bind, so a raw-text scan measures the
    /// commentary — which is not hypothetical: the placeholder-set assertion
    /// below caught exactly that.
    fn statement_body() -> String {
        include_str!("../queries/tick_store_latest.sql")
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row(product_id: &str, observed_at: i64, price: f64) -> TickStoreRow {
        TickStoreRow {
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

    /// A price the engine refuses is still passed on by this reader.
    ///
    /// **Both halves are asserted, because either one alone would be
    /// misleading.** That this reader returns `Some` is only defensible if
    /// something downstream says no — so the test also exercises the predicate
    /// that does, rather than asserting a boundary and trusting prose about the
    /// other side of it. If `Reading::valid` ever stopped covering these values,
    /// this fails here rather than surfacing as a guard that silently never
    /// fires.
    ///
    /// `NaN` is the case that matters most: it defeats *comparison* rather than
    /// arithmetic, so a deviation guard written as `value > tol` would not fire
    /// on it. Zero and negative are included — and are finite, hence the test's
    /// name — because `valid()` covers them on the same terms.
    #[test]
    fn a_price_the_engine_refuses_is_still_passed_on() {
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
        let peg = TickStoreReader::peg(pool);
        assert_eq!(peg.sources(), [SOURCE_KRAKEN]);
        assert_eq!(peg.products(), [USDC_USD_PRODUCT]);
    }

    /// The SQL beside the `.bind()` chain has to agree with it, and it is
    /// runtime-typed, so nothing else checks this until the query runs.
    ///
    /// Compared against the statement's **projected output names** rather than
    /// against its raw text, for the reason the candle reader's twin test
    /// records: the header discusses `confidence` in prose, so a substring sweep
    /// would hold whatever the statement did.
    #[test]
    fn the_query_matches_its_binds_and_its_decoder() {
        let sql = statement_body();

        // Every placeholder `$1..=$n` must appear exactly once, so a duplicated
        // or skipped index fails here. Asserting only the maximum would pass
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
        // than trusting prose to keep it. Checked against the whole SELECT list
        // rather than the resolved output names, because an alias
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
