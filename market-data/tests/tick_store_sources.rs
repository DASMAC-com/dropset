//! The `spot_ticks` reader against a real Postgres.
//!
//! **This is the only gate that can see the statement.** Every query in this
//! crate is runtime-typed — `include_str!`'d and bound positionally, so the crate
//! needs no `DATABASE_URL` at compile time — which means neither clippy nor a
//! compile-time macro can check one. `spot_ticks_latest.sql` is new, and the
//! failure a typo in it produces is not an error: the reader returns no rows, and
//! a consumer reads that as "the collectors are behind". The unit tests pin the
//! statement's *shape* against its decoder; only a database can say it runs.
//!
//! The failure mode worth the container, in one line: a **silently empty leg**,
//! indistinguishable from a slow one.
//!
//! Needs a Docker daemon, so `#[ignore]`d like the fence tests:
//!
//! ```sh
//! cargo test -p dropset-market-data -- --ignored
//! ```
//!
//! **These ARE a merge gate.** The `Tests (Postgres)` job runs
//! `cargo nextest run --run-ignored all` over this crate among four, so every
//! test here executes in CI and in the merge queue — `oanda_direction` is the one
//! target filtered out by name, because it needs a live venue and a credential.
//!
//! That matters for what the `#[ignore]` means: it gates on a **Docker daemon**,
//! not on being optional. Skipping them locally is fine; they will run.
//!
//! It also means these tests carry real weight rather than advisory weight, which
//! is worth stating because the CI-visible set would otherwise be the unit tests
//! alone — and those cannot reach a server. Without this job, all of these
//! mutations would merge green: `DESC` dropped from the ordering, either `WHERE`
//! conjunct dropped, `$1`/`$2` transposed, or a decoder column renamed to one the
//! statement does not project. Each is caught below.

mod common;

use std::time::Duration;

use common::{insert_tick, start_pg};
use dropset_market_data::tick_store::{SpotTickSource, SOURCE_KRAKEN, USDC_USD_PRODUCT};

/// The statement runs, and `DISTINCT ON` takes the newest print per series.
///
/// Both halves matter. That it runs at all is the point of the container; that it
/// takes the *newest* row is the property a stale peg reading would violate
/// silently — the guard would still fire and judge, just on an old number.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_statement_runs_and_takes_the_newest_print() {
    let (_pg, pool) = start_pg().await;

    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_000,
        0.9997,
    )
    .await;
    // A newer print for the same series, which must win. Inserted second so a
    // reader that returned the first row rather than the newest still passes the
    // count assertion below and fails only on the stamp.
    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_060,
        0.9999,
    )
    .await;
    // A series the peg roster does not ask for, so the product filter is load-
    // bearing rather than incidentally satisfied by an empty table.
    insert_tick(&pool, SOURCE_KRAKEN, "EURC-USD", 1_700_000_060, 1.14).await;

    let rows = SpotTickSource::peg(pool.clone())
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

/// An explicit source and product set reaches a series the peg roster does not
/// name.
///
/// The reader holds no roster of its own — the sugar constructor does — and this
/// is what keeps that true. Its twin for the candle reader exists because the
/// old hardcoded shape failed silently: rows were in the table under sources the
/// reader would not ask for, so a leg looked like a collector that had not caught
/// up. Pinned here before this reader can grow the same defect.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn an_explicit_roster_reaches_another_series() {
    let (_pg, pool) = start_pg().await;

    insert_tick(&pool, "coinbase", "EURC-USDC", 1_700_000_060, 1.1410).await;
    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_060,
        0.9999,
    )
    .await;

    let rows = SpotTickSource::new(
        pool.clone(),
        vec!["coinbase".to_string(), SOURCE_KRAKEN.to_string()],
        vec!["EURC-USDC".to_string(), USDC_USD_PRODUCT.to_string()],
    )
    .latest()
    .await
    .expect("read both series");

    assert_eq!(rows.len(), 2, "both requested series must come back");
    let eurc = rows
        .iter()
        .find(|r| r.product_id == "EURC-USDC")
        .expect("the ticker series");
    assert_eq!(eurc.price, 1.1410);
}

/// The two lists are a **cross product**, not a zip of pairs.
///
/// Pinned because the constructor's parameter names invite the opposite reading,
/// and because the mistake is silent rather than loud: a multi-leg caller that
/// assumed pair-wise semantics would bind a leg to another venue's print and
/// still see a plausible number. The fixture is the real case rather than a
/// contrived one — `kraken` genuinely writes both `USDC-USD` and `EURC-USDC`.
///
/// If this reader ever adopts a pair-wise predicate, this test is the one that
/// should fail and be rewritten, which is the point of asserting the current
/// semantics explicitly rather than leaving them implied.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_two_rosters_form_a_cross_product() {
    let (_pg, pool) = start_pg().await;

    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_060,
        0.9999,
    )
    .await;
    insert_tick(&pool, SOURCE_KRAKEN, "EURC-USDC", 1_700_000_060, 1.1410).await;
    insert_tick(&pool, "coinbase", "EURC-USDC", 1_700_000_060, 1.1412).await;

    let rows = SpotTickSource::new(
        pool.clone(),
        vec![SOURCE_KRAKEN.to_string(), "coinbase".to_string()],
        vec![USDC_USD_PRODUCT.to_string(), "EURC-USDC".to_string()],
    )
    .latest()
    .await
    .expect("read the cross product");

    // Three of the four (source, product) combinations exist; `coinbase`/
    // `USDC-USD` was never written. A pair-wise reader would have returned two.
    assert_eq!(
        rows.len(),
        3,
        "every existing combination comes back, not just the zipped pairs"
    );
    assert!(
        rows.iter()
            .any(|r| r.source == SOURCE_KRAKEN && r.product_id == "EURC-USDC"),
        "the off-diagonal kraken/EURC-USDC row is returned — a caller must filter it"
    );
}

/// A source outside the requested set is not returned, even for a requested
/// product.
///
/// The filter is a conjunction, and a reader that dropped the source half would
/// pass every test above — every row in them is one it was asked for.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_source_outside_the_roster_is_not_returned() {
    let (_pg, pool) = start_pg().await;

    // Same product id, written by a venue the peg roster does not name, and
    // NEWER than the one it does — so a reader ignoring its source list returns
    // this row.
    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_000,
        0.9999,
    )
    .await;
    insert_tick(&pool, "coinbase", USDC_USD_PRODUCT, 1_700_000_120, 1.5).await;

    let rows = SpotTickSource::peg(pool.clone())
        .latest()
        .await
        .expect("read the peg series");

    assert_eq!(rows.len(), 1, "only the peg-rostered venue answers");
    assert_eq!(rows[0].source, SOURCE_KRAKEN);
    assert_eq!(
        rows[0].price, 0.9999,
        "a newer print from a venue outside the roster must not become peg truth"
    );
}

/// An empty result is a successful read, not an error.
///
/// The distinction a consumer's store-silence guard keys off: "the collectors are
/// behind" must not read as "the store is gone". The first halts nothing; the
/// second is a halt.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn an_empty_result_is_not_an_error() {
    let (_pg, pool) = start_pg().await;
    let rows = SpotTickSource::peg(pool.clone())
        .latest()
        .await
        .expect("an empty table is a successful read");
    assert!(rows.is_empty());
}

/// An EMPTY roster binds cleanly and returns nothing, rather than erroring.
///
/// Worth pinning because the two outcomes call for different consumer handling
/// and the constructor's docs now promise this one: an untyped-empty-array error
/// would be a loud misconfiguration signal, while a silent empty result is
/// indistinguishable from no data. It is the second, so a consumer assembling a
/// per-market roster has to treat an empty leg list as its own bug.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn an_empty_roster_reads_clean_and_returns_nothing() {
    let (_pg, pool) = start_pg().await;
    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_060,
        0.9999,
    )
    .await;

    let rows = SpotTickSource::new(pool.clone(), vec![], vec![])
        .latest()
        .await
        .expect("an empty bound array is valid SQL, not an error");
    assert!(
        rows.is_empty(),
        "an empty roster matches nothing even though a row exists"
    );
}

/// A row's age comes from the shared convention, through a real round trip.
///
/// The unit tests already pin the ageing, but they construct the row in memory.
/// This proves the value that comes *out of the database* is the one the
/// convention is applied to — that `observed_at` decodes as the stamp rather than
/// as something else that happens to be a `BIGINT`.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_decoded_row_ages_from_its_stored_stamp() {
    let (_pg, pool) = start_pg().await;
    insert_tick(
        &pool,
        SOURCE_KRAKEN,
        USDC_USD_PRODUCT,
        1_700_000_000,
        0.9999,
    )
    .await;

    let rows = SpotTickSource::peg(pool.clone())
        .latest()
        .await
        .expect("read the peg series");
    let reading = rows[0]
        .reading(1_700_000_060, Duration::from_secs(1))
        .expect("an honest row is offered");
    assert_eq!(reading.age, Duration::from_secs(60));
    assert_eq!(reading.value, 0.9999);
}
