//! The estimator's whole path against a real Postgres: read both tables,
//! compose, publish, read the row back.
//!
//! **This is the only gate that can see the SQL.** Every statement on this path
//! is runtime-typed — `include_str!`'d and bound positionally, so the crate needs
//! no `DATABASE_URL` at compile time — which means neither clippy nor a
//! compile-time macro can check one. `spot_ticks_latest.sql` is new here, and a
//! typo in it would pass every check in the repo and then produce an empty peg
//! leg forever: not an error, just a common-mode guard that never fires. The
//! unit tests pin the statement's *shape* against its decoder; only a database
//! can say it runs.
//!
//! It also pins the two things about the composition that no unit test reaches,
//! because both are properties of the round trip rather than of the mapping: that
//! the peg leg genuinely arrives from the **other table**, and that the row
//! `publish` writes survives 0011's and 0014's CHECKs.
//!
//! Needs a Docker daemon, so `#[ignore]`d like the fence tests:
//!
//! ```sh
//! cargo test -p dropset-market-data -- --ignored
//! ```
//!
//! **Operator-run, not a merge gate** — CI's only `--run-ignored` invocation
//! selects two other crates, so nothing here executes in the merge queue. The
//! wiring is tracked separately.

mod common;

use std::time::Duration;

use common::{insert_bucket, insert_tick, start_pg};
use dropset_feeds::now_secs;
use dropset_market_data::estimator::{Estimator, EstimatorMarket, Ticked, MVP_MARKETS};
use dropset_market_data::tick_store::{SpotTickSource, SOURCE_KRAKEN, USDC_USD_PRODUCT};
use sqlx::{PgPool, Row};

/// One published row, read back through the columns a consumer reads.
struct PublishedRow {
    fair: Option<f64>,
    anchor: String,
    regime: String,
    health: String,
    basis: Option<f64>,
    /// The USDC/USD common-mode guard — the **only** thing the peg leg decides.
    /// See `the_peg_leg_comes_from_the_tick_table` for why that matters here.
    usdc_breach: bool,
    leg_stale_tape_secs: i64,
    leg_stale_reference_secs: i64,
}

async fn read_published(pool: &PgPool, product_id: &str) -> Option<PublishedRow> {
    let row = sqlx::query(
        "SELECT fair, anchor, regime, health, basis, usdc_breach,
                leg_stale_tape_secs, leg_stale_reference_secs
         FROM fair_price
         WHERE product_id = $1
         ORDER BY ts DESC
         LIMIT 1",
    )
    .bind(product_id)
    .fetch_optional(pool)
    .await
    .expect("read fair_price back")?;
    Some(PublishedRow {
        fair: row.try_get("fair").expect("fair"),
        anchor: row.try_get("anchor").expect("anchor"),
        regime: row.try_get("regime").expect("regime"),
        health: row.try_get("health").expect("health"),
        basis: row.try_get("basis").expect("basis"),
        usdc_breach: row.try_get("usdc_breach").expect("usdc_breach"),
        leg_stale_tape_secs: row
            .try_get("leg_stale_tape_secs")
            .expect("leg_stale_tape_secs"),
        leg_stale_reference_secs: row
            .try_get("leg_stale_reference_secs")
            .expect("leg_stale_reference_secs"),
    })
}

/// Write every leg fresh, as of `now`, for the whole MVP roster.
async fn seed_all_legs(pool: &PgPool, now: i64) {
    // FX anchors, from two tapes so the leg is corroborated rather than single.
    for (currency, rate) in [("EUR", 1.1400), ("AUD", 0.7200), ("CAD", 0.7300)] {
        let product = format!("{currency}-USD");
        insert_bucket(pool, "oanda", &product, now, rate).await;
        insert_bucket(pool, "twelvedata", &product, now, rate + 0.0002).await;
    }
    // EURC's crypto reference — the one observed basis on the roster.
    insert_bucket(pool, "coinbase", "EURC-USDC", now, 1.1410).await;
    // The peg leg, in the OTHER table. This is the row the whole second reader
    // exists for.
    insert_tick(pool, SOURCE_KRAKEN, USDC_USD_PRODUCT, now, 0.9999).await;
}

/// The `spot_ticks` statement runs, and returns the series it was asked for.
///
/// Separate from the composition test on purpose: if the SQL is wrong, this
/// fails with "the statement did not run" rather than the composition failing
/// with "the peg leg was empty", which reads like a mapping bug.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_tick_statement_runs_and_returns_the_peg_series() {
    let (_pg, pool) = start_pg().await;

    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_000,
        0.9997,
    )
    .await;
    // A newer print for the same series, which must win.
    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_060,
        0.9999,
    )
    .await;
    // A series the peg roster does not ask for.
    insert_tick(&pool, SOURCE_KRAKEN, "EURC-USD", 1_700_000_060, 1.14).await;

    let rows = SpotTickSource::peg("test:peg", pool.clone())
        .latest()
        .await
        .expect("the spot_ticks statement runs");

    assert_eq!(
        rows.len(),
        1,
        "one row per series, and one series asked for"
    );
    assert_eq!(rows[0].product_id, USDC_USD_PRODUCT);
    assert_eq!(rows[0].source, SOURCE_KRAKEN);
    assert_eq!(
        rows[0].observed_at, 1_700_000_060,
        "DISTINCT ON must take the newest print, not the first inserted"
    );
    assert_eq!(rows[0].price, 0.9999);
}

/// An empty tick table is a successful read, not an error.
///
/// The distinction the store-silence guard keys off: "the collectors are behind"
/// must not read as "the store is gone".
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn an_empty_tick_table_is_not_an_error() {
    let (_pg, pool) = start_pg().await;
    let rows = SpotTickSource::peg("test:peg", pool.clone())
        .latest()
        .await
        .expect("an empty table is a successful read");
    assert!(rows.is_empty());
}

/// One full tick: every leg present, every market published, and the row that
/// lands survives the schema's CHECKs.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_full_tick_publishes_every_market() {
    let (_pg, pool) = start_pg().await;
    let now = now_secs();
    seed_all_legs(&pool, now).await;

    let mut estimator = Estimator::new(pool.clone(), MVP_MARKETS.to_vec(), Duration::from_secs(15))
        .expect("the MVP roster is constructible");
    let outcome = estimator
        .tick_once()
        .await
        .expect("a seeded tick must not halt");
    assert_eq!(
        outcome,
        Ticked::Published,
        "the store answered, so this is not a cached tick"
    );

    for market in MVP_MARKETS {
        let row = read_published(&pool, market.product_id)
            .await
            .unwrap_or_else(|| panic!("{} published no row", market.product_id));
        assert!(
            row.fair.is_some_and(|f| f > 0.0),
            "{} published no usable mid",
            market.product_id
        );
        assert_eq!(
            row.anchor, "fx",
            "{} must anchor on the FX leg with every leg fresh",
            market.product_id
        );
        // **No MVP market composes as `ok`, with every leg fresh**, and that is a
        // roster fact rather than a defect — worth asserting so it is a decision
        // on the record rather than a surprise on a dashboard. EURC's basis rests
        // on one venue, so it is `uncorroborated`; AUDD and CADC are `fx_pinned`.
        // Both gate as `unverified`: quoted, and not described as corroborated.
        assert_eq!(
            row.health, "unverified",
            "{} composes on an uncorroborated or pinned basis",
            market.product_id
        );
        // Every leg is inside its band, so the common-mode guard is quiet.
        assert!(!row.usdc_breach, "{}", market.product_id);

        // 0014's per-row bounds have to be the ones the composition was resolved
        // at. Its CHECK is `tape > 0 AND reference >= tape`, so a row landing at
        // all already proves both; asserting them here states what the columns
        // are expected to *mean*, so a future change that wrote a placeholder
        // satisfying the CHECK still fails.
        assert!(
            row.leg_stale_tape_secs > 0,
            "{}: the tape bound must be the engine's, not a zero",
            market.product_id
        );
        assert!(
            row.leg_stale_reference_secs >= row.leg_stale_tape_secs,
            "{}: 0014 orders the pair",
            market.product_id
        );
    }
}

/// The peg leg reaches the composition **from `spot_ticks`**, observed through
/// the one output it decides: `usdc_breach`.
///
/// **The peg leg is not in the basis, and reading the crate's headline formula as
/// though it were is the trap here.** `basis = EMA of (token/fiat ÷ USDC/USD)`
/// describes the model; `Legs::observed_basis` divides the crypto reference by
/// the FX anchor and does not consult `usdc_usd` at all. So a test asserting the
/// peg leg by watching `basis` move asserts nothing — it passes identically
/// whether the second reader works or returns nothing, which is exactly the
/// silent-empty-leg failure this file exists to catch. Measured: that version of
/// this test reported the same basis to fifteen decimal places both ways.
///
/// The peg leg's whole observable effect is the portfolio-wide common-mode guard,
/// so the print below sits outside the configured band (0.97–1.03) — a print
/// *inside* it produces `usdc_breach = false` exactly like an absent leg, and
/// would be the same vacuous test in a second disguise.
///
/// Asserted by difference — the same seeded world twice — because a single
/// populated case cannot distinguish "the guard fired" from "the guard is
/// always on".
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_peg_leg_comes_from_the_tick_table() {
    async fn tick_with_peg(peg: Option<f64>) -> PublishedRow {
        let (_pg, pool) = start_pg().await;
        let now = now_secs();
        insert_bucket(&pool, "oanda", "EUR-USD", now, 1.1400).await;
        insert_bucket(&pool, "twelvedata", "EUR-USD", now, 1.1402).await;
        insert_bucket(&pool, "coinbase", "EURC-USDC", now, 1.1410).await;
        if let Some(price) = peg {
            insert_tick(&pool, SOURCE_KRAKEN, USDC_USD_PRODUCT, now, price).await;
        }
        let eurc: EstimatorMarket = MVP_MARKETS[0];
        let mut estimator = Estimator::new(pool.clone(), vec![eurc], Duration::from_secs(15))
            .expect("constructible");
        estimator.tick_once().await.expect("must not halt");
        read_published(&pool, eurc.product_id)
            .await
            .expect("published")
    }

    // No peg row at all: the guard cannot fire, which the engine treats as
    // exactly that rather than as a fault.
    let without = tick_with_peg(None).await;
    assert!(
        !without.usdc_breach,
        "an absent peg leg cannot breach — the guard simply has nothing to judge"
    );

    // A peg print well below the 0.97 floor. If the tick reader returns nothing,
    // this row is invisible and the guard stays quiet.
    let with = tick_with_peg(Some(0.9000)).await;
    assert!(
        with.usdc_breach,
        "the peg leg did not reach the composition: a 0.90 USDC/USD print is far \
         outside the 0.97-1.03 band, so `usdc_breach` must be set. The row is in \
         `spot_ticks`, so a quiet guard means the tick reader returned nothing."
    );

    // The peg decides the guard, not the regime or the mid — so a breach must not
    // silently change what the market is anchored on. Stated here so a future
    // change that darks a market on a breach fails visibly rather than passing
    // the assertion above.
    assert_eq!(without.regime, with.regime);
    assert_eq!(without.anchor, with.anchor);
    assert_eq!(
        without.basis, with.basis,
        "the peg is not a basis input; `observed_basis` divides crypto by fx"
    );
}

/// A pinned market publishes on the FX anchor with no crypto leg at all.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_pinned_market_publishes_unverified() {
    let (_pg, pool) = start_pg().await;
    let now = now_secs();
    let audd: EstimatorMarket = MVP_MARKETS[1];
    assert!(
        audd.pinned_basis.is_some(),
        "this test is about a pinned market"
    );

    insert_bucket(&pool, "oanda", "AUD-USD", now, 0.7200).await;
    // A crypto row for its pair EXISTS — the candle collector rosters
    // `AUDD-USDC` — so a market that priced off it would be visible here.
    insert_bucket(&pool, "coinbase", "AUDD-USDC", now, 0.9999).await;

    let mut estimator =
        Estimator::new(pool.clone(), vec![audd], Duration::from_secs(15)).expect("constructible");
    estimator.tick_once().await.expect("must not halt");

    let row = read_published(&pool, audd.product_id)
        .await
        .expect("published");
    assert_eq!(row.regime, "fx_pinned");
    assert_eq!(
        row.health, "unverified",
        "a pinned basis is quoted but not described as corroborated"
    );
    let fair = row.fair.expect("a pinned market still composes a mid");
    assert!(
        (fair - 0.72).abs() < 1e-6,
        "fair must be the FX anchor times the 1:1 pin, not the stray crypto row \
         at 0.9999 — got {fair}",
    );
}

/// Two ticks in a row both publish, at different stamps.
///
/// `fair_price`'s primary key is `(product_id, ts)` in whole seconds, so this is
/// the shape that would collide if the estimator ticked faster than once a
/// second. A second row at a later stamp is what proves the series accumulates
/// rather than overwriting.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn consecutive_ticks_accumulate_rows() {
    let (_pg, pool) = start_pg().await;
    let now = now_secs();
    let eurc: EstimatorMarket = MVP_MARKETS[0];
    insert_bucket(&pool, "oanda", "EUR-USD", now, 1.1400).await;
    insert_tick(&pool, SOURCE_KRAKEN, USDC_USD_PRODUCT, now, 0.9999).await;

    let mut estimator =
        Estimator::new(pool.clone(), vec![eurc], Duration::from_secs(1)).expect("constructible");
    estimator.tick_once().await.expect("first tick");
    // Past the one-second key granularity, so the second tick takes a new stamp.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    estimator.tick_once().await.expect("second tick");

    let count: i64 = sqlx::query("SELECT count(*) AS n FROM fair_price WHERE product_id = $1")
        .bind(eurc.product_id)
        .fetch_one(&pool)
        .await
        .expect("count the published rows")
        .try_get("n")
        .expect("n");
    assert_eq!(count, 2, "each tick publishes its own row");
}
