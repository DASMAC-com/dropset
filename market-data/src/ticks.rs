//! Spot ticks: the point-in-time print, and the one path that persists it.
//!
//! Three venues feed the `spot_ticks` table and none of them yields the same
//! record type: Kraken's batched ticker gives `pair → price`, Pyth Hermes gives
//! `key → (price, confidence, publish_time)`, and the Coinbase ticker gives one
//! `(product_id, price)` pair per poll. Rather than write three
//! [`StoreWriter`]s — three copies of one INSERT, drifting — this module
//! defines the single row shape ([`Tick`]), the single writer
//! ([`TickWriter`]), and a [`TickSource`] adapter that maps whatever a venue
//! yields onto it.
//!
//! The mapping is a closure the collector supplies, so this module knows
//! nothing about any venue: the pair-name translation Kraken needs and the
//! confidence Pyth carries stay with the collector that understands them,
//! which is the same division [`crate::store`] already keeps for candles.

use anyhow::Result;
use async_trait::async_trait;
use dropset_feeds::{now_secs, Batch, Source, StoreWriter};

/// The per-venue starting points a tick collector's binary supplies, since a
/// venue's API root and poll budget are properties of the venue rather than of
/// the deployment.
#[derive(Clone, Debug)]
pub struct TickDefaults {
    pub base_url: &'static str,
    /// Seconds between polls. Unlike a candle feed there is no bucket width to
    /// align to, so this *is* the resolution of the stored series.
    pub poll_interval_secs: u64,
}

/// One tick collector's configuration.
///
/// Deliberately smaller than [`crate::fx::FxConfig`]: a ticker has no window,
/// no backfill, and no request cap, so carrying those fields would invite
/// someone to set them and expect something to happen. It also holds **no
/// roster** — the two keyless venues read theirs from the environment while the
/// Pyth collector reads its own from the store, and folding both into one type
/// would leave a field that is meaningless for one of them.
#[derive(Clone, Debug)]
pub struct TickConfig {
    pub database_url: String,
    pub base_url: String,
    pub poll_interval_secs: u64,
}

impl TickConfig {
    /// Read the collector's configuration, starting from its venue's defaults.
    ///
    /// `BASE_URL` rather than a venue-prefixed name: one process serves one
    /// venue, so there is nothing to disambiguate within it.
    pub fn from_env(defaults: &TickDefaults) -> Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
        let base_url = std::env::var("BASE_URL").unwrap_or_else(|_| defaults.base_url.to_string());
        let poll_interval_secs = std::env::var("POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(defaults.poll_interval_secs);
        Ok(Self {
            database_url,
            base_url,
            poll_interval_secs,
        })
    }
}

/// One observation of one product's price.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    /// The canonical product id this reading is stored under.
    pub product_id: String,
    /// Epoch second the reading is attributed to — the venue's publish time
    /// where it publishes one, else the poll second. See the `spot_ticks`
    /// migration for why that choice is what makes a re-poll idempotent.
    pub observed_at: i64,
    pub price: f64,
    /// Symmetric confidence half-width, for a venue that publishes one.
    ///
    /// `None` means the venue has no confidence notion, **not** zero — a zero
    /// would read as perfect certainty. The table constrains this to
    /// `NULL OR > 0` and [`TickWriter`] normalizes into that shape.
    pub confidence: Option<f64>,
}

/// Writes [`Tick`] records for one venue into `spot_ticks`. The source label is
/// constant per collector, so it lives here rather than on every record.
pub struct TickWriter {
    source: String,
}

impl TickWriter {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

#[async_trait]
impl StoreWriter for TickWriter {
    type Record = Tick;

    async fn write_batch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        records: &[Tick],
    ) -> Result<u64> {
        let mut written = 0;
        for t in records {
            let res = sqlx::query(include_str!("../queries/spot_tick_insert.sql"))
                .bind(&self.source)
                .bind(&t.product_id)
                .bind(t.observed_at)
                .bind(t.price)
                .bind(normalize_confidence(t.confidence))
                .execute(&mut **tx)
                .await?;
            written += res.rows_affected();
        }
        Ok(written)
    }
}

/// Coerce a confidence into the shape the table's constraint admits.
///
/// The venue adapters already reject a zero, negative, or non-finite
/// half-width as malformed, so this should never change a value. It exists
/// because the alternative failure mode is severe and asymmetric: a value that
/// *did* slip through would fail the `CHECK`, abort the batch transaction, and
/// put the collector in a crash loop that no amount of restarting clears — a
/// venue-data problem escalated into an outage. Mapping it to "unknown"
/// degrades one field of one tick instead, which is also the honest reading.
fn normalize_confidence(confidence: Option<f64>) -> Option<f64> {
    confidence.filter(|c| c.is_finite() && *c > 0.0)
}

/// Adapt a venue source into a [`Tick`] source by mapping each of its records.
///
/// One venue record commonly becomes several ticks — a batched ticker prices
/// its whole roster in one record — so the mapping returns a `Vec` and the
/// results are flattened.
pub struct TickSource<S, F> {
    inner: S,
    map: F,
}

impl<S, F> TickSource<S, F> {
    /// Wrap `inner`, mapping each of its records into ticks with `map`.
    ///
    /// `map` is handed the poll's epoch second so a venue that publishes no
    /// timestamp of its own has one to attribute the reading to; a venue that
    /// does publish one should prefer it and ignore this.
    pub fn new(inner: S, map: F) -> Self {
        Self { inner, map }
    }
}

#[async_trait]
impl<S, F> Source for TickSource<S, F>
where
    S: Source + Send,
    S::Record: Send + Sync,
    // `FnMut`, not `Fn`: a mapping over a venue that omits what it could not
    // price has to remember what it has seen to tell a typo from an outage
    // (see [`SilenceWatch`]), which means owning state across polls.
    F: FnMut(&S::Record, i64) -> Vec<Tick> + Send,
{
    type Record = Tick;

    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn next(&mut self) -> Result<Batch<Tick>> {
        let batch = self.inner.next().await?;
        // One clock reading per poll, not per record: every tick in a batch
        // was observed by the same request, and stamping them individually
        // would spread one observation across several seconds.
        let observed_at = now_secs();
        let map = &mut self.map;
        let records: Vec<Tick> = batch
            .records
            .iter()
            .flat_map(|record| map(record, observed_at))
            .collect();
        // **The inner cursor is deliberately NOT forwarded.** Every tick source
        // today is a snapshot endpoint with no resume position, so there is
        // none to forward — but the reason this is a drop rather than a
        // pass-through matters if that ever changes: the mapping closure is
        // allowed to drop records (the Kraken mapping omits a response key the
        // roster does not cover), so forwarding a resume position would commit
        // a cursor *past* data that was never written, turning the store
        // sink's at-least-once contract into at-most-once. Dropping the cursor
        // degrades to re-fetching, which the idempotent insert absorbs;
        // forwarding it would lose rows silently. A paged tick source would
        // need the mapping to be provably total before this could change.
        Ok(Batch::new(records).with_caught_up(batch.caught_up))
    }
}

/// Watches for a configured product that the venue never prices, and says so.
///
/// **Why this is not optional.** The batched adapters *omit* a symbol they got
/// no answer for rather than erroring — deliberately, so one unquoted pair
/// cannot take a whole roster down. The cost is that every misconfiguration
/// looks exactly like a venue outage: a mistyped Pyth feed id, a Kraken pair
/// spelled the way we name it rather than the way Kraken does, or a currency
/// the venue simply does not publish all produce the same thing, which is
/// silence. Nothing errors, nothing is logged, and the missing series reads as
/// "the venue was down" — or, worse, is not noticed at all.
///
/// So the collector has to assert the difference itself: a product configured
/// but *never once* priced, after enough polls that a transient gap is ruled
/// out, is a configuration error and is reported as one. A product that priced
/// and then stopped is a different event and is deliberately not this type's
/// business — that is what the store's own coverage shows.
pub struct SilenceWatch {
    configured: Vec<String>,
    seen: std::collections::HashSet<String>,
    polls: u32,
    warn_after: u32,
    warned: bool,
}

impl SilenceWatch {
    /// Watch `configured` product ids, reporting after `warn_after` polls.
    ///
    /// `warn_after` wants to be a handful of polls, not one: a venue can miss a
    /// single response for a symbol it usually prices, and warning on the first
    /// poll would cry wolf on every startup.
    pub fn new(configured: Vec<String>, warn_after: u32) -> Self {
        Self {
            configured,
            seen: std::collections::HashSet::new(),
            polls: 0,
            warn_after: warn_after.max(1),
            warned: false,
        }
    }

    /// Record one poll's ticks, and report once the threshold is reached.
    ///
    /// **The roster-error claim is only made once the venue has demonstrably
    /// answered for something**, and that condition is the whole design. The
    /// message tells an operator to check their configuration *rather than* the
    /// venue's uptime, which is a confident claim — and it is only warranted
    /// when some other product on the same roster priced, proving the venue is
    /// reachable and answering. If **nothing** has ever priced, a roster full
    /// of typos and a venue that is simply down look identical, so this says
    /// the weaker true thing instead and does **not** latch: the next poll can
    /// still resolve it. Latching a wrong diagnosis would leave a startup-window
    /// outage permanently misreported as a config error.
    pub fn observe(&mut self, ticks: &[Tick]) {
        for tick in ticks {
            self.seen.insert(tick.product_id.clone());
        }
        self.polls += 1;
        if self.warned || self.polls < self.warn_after {
            return;
        }
        // Owned, so the borrow of `self` ends before `warned` is assigned.
        let silent: Vec<String> = self.silent().into_iter().map(str::to_string).collect();
        if silent.is_empty() {
            // Everything configured has priced at least once; there is nothing
            // left for this watch to say, ever.
            self.warned = true;
            return;
        }
        if self.seen.is_empty() {
            tracing::warn!(
                products = %silent.join(","),
                polls = self.polls,
                "no configured product has priced yet — the venue may be \
                 unreachable, so this is not yet attributable to the roster"
            );
            return;
        }
        self.warned = true;
        tracing::warn!(
            products = %silent.join(","),
            polls = self.polls,
            "configured products never priced by this venue — check the roster \
             spelling or the feed id, not the venue's uptime"
        );
    }

    /// The configured products that have never been priced.
    fn silent(&self) -> Vec<&str> {
        self.configured
            .iter()
            .map(String::as_str)
            .filter(|p| !self.seen.contains(*p))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_confidence_becomes_unknown_rather_than_failing_the_batch() {
        // The asymmetry this protects: a CHECK violation would abort the
        // transaction and crash-loop the collector.
        assert_eq!(normalize_confidence(Some(0.0)), None);
        assert_eq!(normalize_confidence(Some(-1.0)), None);
        assert_eq!(normalize_confidence(Some(f64::NAN)), None);
        assert_eq!(normalize_confidence(Some(f64::INFINITY)), None);
        assert_eq!(normalize_confidence(None), None);
        // A real half-width is untouched.
        assert_eq!(normalize_confidence(Some(1e-5)), Some(1e-5));
    }

    fn tick(product_id: &str) -> Tick {
        Tick {
            product_id: product_id.to_string(),
            observed_at: 1,
            price: 1.0,
            confidence: None,
        }
    }

    #[test]
    fn a_product_that_never_prices_is_reported_as_configuration_not_uptime() {
        let mut watch = SilenceWatch::new(vec!["EUR-USD".to_string(), "TYPO-USD".to_string()], 3);
        watch.observe(&[tick("EUR-USD")]);
        watch.observe(&[tick("EUR-USD")]);
        assert_eq!(watch.silent(), vec!["TYPO-USD"]);
        assert!(!watch.warned, "must not warn before the threshold");
        watch.observe(&[tick("EUR-USD")]);
        assert!(watch.warned, "the third poll reaches the threshold");
    }

    #[test]
    fn a_transient_gap_does_not_read_as_a_typo() {
        // The reason the threshold exists: a venue can miss one response for a
        // symbol it usually prices, and warning on the first poll would fire on
        // every startup.
        let mut watch = SilenceWatch::new(vec!["EUR-USD".to_string()], 3);
        watch.observe(&[]);
        watch.observe(&[tick("EUR-USD")]);
        watch.observe(&[]);
        assert!(watch.silent().is_empty());
    }

    #[test]
    fn a_full_roster_reports_nothing() {
        let mut watch = SilenceWatch::new(vec!["EUR-USD".to_string()], 1);
        watch.observe(&[tick("EUR-USD")]);
        assert!(watch.silent().is_empty());
    }

    #[test]
    fn a_venue_that_never_answers_is_not_blamed_on_the_roster() {
        // The misdiagnosis this prevents: if NOTHING has ever priced, a roster
        // full of typos and a venue that is simply down are indistinguishable,
        // so the confident "check your config, not the venue's uptime" claim is
        // unwarranted. Crucially the watch must NOT latch here, or a
        // startup-window outage stays misreported for the life of the process.
        let mut watch = SilenceWatch::new(vec!["EUR-USD".to_string(), "GBP-USD".to_string()], 3);
        for _ in 0..5 {
            watch.observe(&[]);
        }
        assert!(!watch.warned, "must stay open while the venue is silent");

        // Once the venue proves it is answering, the remaining silent product
        // IS attributable to the roster — and now it latches.
        watch.observe(&[tick("EUR-USD")]);
        assert!(watch.warned, "one print makes the rest attributable");
        assert_eq!(watch.silent(), vec!["GBP-USD"]);
    }

    #[test]
    fn a_roster_that_fully_prices_never_warns_again() {
        let mut watch = SilenceWatch::new(vec!["EUR-USD".to_string()], 2);
        watch.observe(&[tick("EUR-USD")]);
        watch.observe(&[tick("EUR-USD")]);
        assert!(watch.warned, "nothing left to say, so it latches closed");
        assert!(watch.silent().is_empty());
    }
}
