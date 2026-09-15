//! The fair-price insert, run against a real Postgres, column by column.
//!
//! **Why this needs its own test.** `queries/fair_price_insert.sql` is a
//! runtime, string-typed statement loaded through `include_str!` and driven by a
//! chain of fifteen positional `.bind()` calls in
//! [`dropset_market_data::fair_price::publish`]. Nothing else in the repo
//! executes it: clippy cannot see inside it, there is no compile-time
//! `DATABASE_URL` macro to check it, and `db-schema`'s fence tests reach
//! `fair_price` by hand-written INSERT if at all.
//!
//! The specific hazard is **transposition, not typo**. A misspelled column
//! fails loudly the first time it runs. But `fair_price` has four adjacent
//! BOOLEAN columns (`basis_outlier`, `uncertain`, `basis_breach`,
//! `usdc_breach`), two nullable DOUBLE PRECISION columns (`fair`, `basis`), and
//! now four BIGINTs — so swapping two members of any of those groups, in the
//! column list or in the bind chain, is accepted silently by Postgres. No type
//! error, no failed test, and the wrong values go into a table whose rows are
//! never revised. A `usdc_breach` reported as an `uncertain` is a
//! portfolio-wide halt condition rendered as "quote, but widen".
//!
//! **The booleans are therefore pinned across four rows, one-hot.** Within a
//! single row, two boolean columns holding the same value are interchangeable by
//! construction, so no single row — however carefully chosen — can detect every
//! pairwise transposition among four of them. Setting exactly one true per row
//! makes every pair differ in at least one row, so any swap moves a `true` to a
//! column that must read `false`. One-hot is *sufficient* rather than unique
//! (`1100` plus `1010` separates all six pairs too); it is chosen because one
//! row per column is the shape that says which column each row is about.
//!
//! Needs a Docker daemon, so `#[ignore]`d like the fence tests and the
//! roster-registration test beside it:
//!
//! ```sh
//! cargo test -p dropset-market-data -- --ignored
//! ```
//!
//! **These are a merge gate.** The Tests (Postgres) job's `--run-ignored all`
//! invocation selects `dropset-market-data`, so the transposition proof below
//! runs in the merge queue, and on any PR that trips the workflow's `code`
//! filter. Note the consequence: an `#[ignore]`d test is skipped by a bare
//! local `cargo test`, so unless you run the command above you will first
//! learn of a failure from CI. Run it before pushing a change that touches
//! this file.

mod common;

use std::time::Duration;

use common::start_pg;
use dropset_fair_value::{
    Anchor, Candidates, ClockCtx, FairValue, FairValueConfig, FairValueEngine, Health,
    LegStaleness, Legs, Reading, Regime,
};
use dropset_market_data::fair_price::publish;
use sqlx::{PgPool, Row};

/// One real composition, to be overridden field by field below.
///
/// Produced by the engine rather than assembled literally so the `LegReport` and
/// `FusionReport` members are whatever the engine really builds. The test
/// overrides only the fields the statement serializes; it is not asserting
/// anything about what the engine computed.
fn composed() -> FairValue {
    let age = Duration::from_secs(1);
    let mut engine = FairValueEngine::new(FairValueConfig::default());
    let legs = Legs {
        fx: Candidates::none().push_trusted("pyth-hermes", Some(Reading::new(1.14, age))),
        crypto_usdc: Candidates::none()
            .push("coinbase", Some(Reading::new(1.141, age)))
            .push("kraken", Some(Reading::new(1.142, age))),
        usdc_usd: Candidates::none().push("kraken", Some(Reading::new(1.0, age))),
        static_usd: 1.14,
    };
    engine.compose(legs, Duration::from_secs(5), ClockCtx::in_session())
}

/// Every column the statement writes except the two that key the read back.
///
/// `ts` and `product_id` are absent because `read_row` looks the row up by them,
/// which pins them a different way: a transposition involving either means no
/// row matches and `fetch_one` fails. The diagnostic is worse than a named
/// column mismatch — an opaque "read back EUR-USD at ..." — so if that ever
/// fires, suspect the key columns first.
///
/// Read by name rather than by index deliberately: reading positionally would
/// reproduce the very assumption under test, so a transposed column list would
/// agree with a transposed read and the test would pass.
struct StoredRow {
    fair: Option<f64>,
    anchor: String,
    regime: String,
    degrade: Option<String>,
    health: String,
    basis: Option<f64>,
    basis_age_secs: Option<i64>,
    basis_outlier: bool,
    uncertain: bool,
    basis_breach: bool,
    usdc_breach: bool,
    leg_stale_tape_secs: i64,
    leg_stale_reference_secs: i64,
}

async fn read_row(pool: &PgPool, product_id: &str, ts: i64) -> StoredRow {
    let row = sqlx::query(
        "SELECT fair, anchor, regime, degrade, health, basis, basis_age_secs,
                basis_outlier, uncertain, basis_breach, usdc_breach,
                leg_stale_tape_secs, leg_stale_reference_secs
             FROM fair_price WHERE product_id = $1 AND ts = $2",
    )
    .bind(product_id)
    .bind(ts)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("read back {product_id} at {ts}: {e}"));

    StoredRow {
        fair: row.try_get("fair").expect("fair"),
        anchor: row.try_get("anchor").expect("anchor"),
        regime: row.try_get("regime").expect("regime"),
        degrade: row.try_get("degrade").expect("degrade"),
        health: row.try_get("health").expect("health"),
        basis: row.try_get("basis").expect("basis"),
        basis_age_secs: row.try_get("basis_age_secs").expect("basis_age_secs"),
        basis_outlier: row.try_get("basis_outlier").expect("basis_outlier"),
        uncertain: row.try_get("uncertain").expect("uncertain"),
        basis_breach: row.try_get("basis_breach").expect("basis_breach"),
        usdc_breach: row.try_get("usdc_breach").expect("usdc_breach"),
        leg_stale_tape_secs: row
            .try_get("leg_stale_tape_secs")
            .expect("leg_stale_tape_secs"),
        leg_stale_reference_secs: row
            .try_get("leg_stale_reference_secs")
            .expect("leg_stale_reference_secs"),
    }
}

/// Distinguishable bounds: unequal to each other, unequal to every other
/// numeric the test binds, and ordered so 0014's CHECK accepts them. Equal
/// bounds would make the two columns interchangeable, which is the defect.
const STALE: LegStaleness = LegStaleness {
    tape: Duration::from_secs(31),
    reference: Duration::from_secs(97),
};

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn every_column_round_trips_with_a_distinguishable_value() {
    let (_pg, pool) = start_pg().await;

    // Every scalar distinct from every other, so a transposition inside a type
    // group changes a read value rather than landing on a coincidence. The two
    // DOUBLE PRECISIONs differ in magnitude, not just in digits, and neither
    // equals a bound or an age.
    let fv = FairValue {
        fair: Some(1.234_567),
        anchor: Anchor::Fx,
        regime: Regime::Normal,
        basis: Some(0.987_654),
        basis_age: Some(Duration::from_secs(42)),
        basis_outlier: true,
        uncertain: false,
        basis_breach: false,
        usdc_breach: false,
        health: Health::Ok,
        ..composed()
    };

    let ts = 1_700_000_003;
    let new = publish(&pool, ts, "EUR-USD", &fv, STALE)
        .await
        .expect("publish a composition");
    assert!(new, "a first write for this pair and stamp must be new");

    let got = read_row(&pool, "EUR-USD", ts).await;

    assert_eq!(got.fair, Some(1.234_567), "fair");
    assert_eq!(got.basis, Some(0.987_654), "basis");
    assert_eq!(got.basis_age_secs, Some(42), "basis_age_secs");
    assert_eq!(got.anchor, "fx", "anchor");
    assert_eq!(got.regime, "normal", "regime");
    assert_eq!(
        got.degrade, None,
        "degrade is NULL outside a degraded regime"
    );
    assert_eq!(got.health, "ok", "health");
    assert_eq!(got.leg_stale_tape_secs, 31, "leg_stale_tape_secs");
    assert_eq!(got.leg_stale_reference_secs, 97, "leg_stale_reference_secs");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn each_guard_flag_lands_in_its_own_column() {
    let (_pg, pool) = start_pg().await;
    let base = composed();

    // One row per boolean, that boolean alone true. See the module note on why
    // a single row cannot cover this.
    const FLAGS: [&str; 4] = ["basis_outlier", "uncertain", "basis_breach", "usdc_breach"];

    for (i, name) in FLAGS.iter().enumerate() {
        let mut fv = FairValue {
            basis_outlier: false,
            uncertain: false,
            basis_breach: false,
            usdc_breach: false,
            ..base
        };
        // Set by name rather than through a closure table so the mapping from
        // the asserted name to the field is written once and visibly.
        match *name {
            "basis_outlier" => fv.basis_outlier = true,
            "uncertain" => fv.uncertain = true,
            "basis_breach" => fv.basis_breach = true,
            "usdc_breach" => fv.usdc_breach = true,
            other => unreachable!("no such guard flag: {other}"),
        }

        // A distinct stamp per case, so all four rows coexist under the
        // (product_id, ts) key and one failure does not mask the others.
        let ts = 1_700_000_100 + i as i64;
        publish(&pool, ts, "AUD-USD", &fv, STALE)
            .await
            .unwrap_or_else(|e| panic!("publish the {name} case: {e}"));

        let got = read_row(&pool, "AUD-USD", ts).await;
        let read = [
            ("basis_outlier", got.basis_outlier),
            ("uncertain", got.uncertain),
            ("basis_breach", got.basis_breach),
            ("usdc_breach", got.usdc_breach),
        ];
        for (col, value) in read {
            assert_eq!(
                value,
                col == *name,
                "with only {name} set, {col} read back {value} — the bind chain \
                 and the column list disagree, so a guard flag is landing in \
                 the wrong column"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_paused_composition_stores_nulls_rather_than_zeroes() {
    let (_pg, pool) = start_pg().await;

    // The nullable columns are the ones a transposition can hide in most
    // quietly, because a NULL landing in the wrong nullable column still reads
    // as a legitimate value. 0011 is explicit that NULL means unknown and never
    // zero — a panel rendering a paused tick as 0 draws it as a collapse.
    let fv = FairValue {
        fair: None,
        basis: None,
        basis_age: None,
        regime: Regime::Paused,
        anchor: Anchor::None,
        health: Health::Pause,
        ..composed()
    };

    let ts = 1_700_000_200;
    publish(&pool, ts, "CAD-USD", &fv, STALE)
        .await
        .expect("publish a paused composition");

    let got = read_row(&pool, "CAD-USD", ts).await;
    assert_eq!(got.fair, None, "a paused regime publishes no mid");
    assert_eq!(got.basis, None, "no basis without an FX anchor");
    assert_eq!(got.basis_age_secs, None, "no age for an absent basis");
    assert_eq!(got.regime, "paused", "regime");
    // Still NOT NULL, and still the bounds the composition was judged at: a
    // paused tick was judged too, and that is what made it paused.
    assert_eq!(got.leg_stale_tape_secs, 31, "bounds are recorded even so");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn republishing_the_same_pair_and_stamp_reports_not_new() {
    let (_pg, pool) = start_pg().await;
    let fv = composed();
    let ts = 1_700_000_300;

    assert!(
        publish(&pool, ts, "EUR-USD", &fv, STALE)
            .await
            .expect("first publish"),
        "the first write is new"
    );
    assert!(
        !publish(&pool, ts, "EUR-USD", &fv, STALE)
            .await
            .expect("second publish must not error"),
        "a repeat write must report false — the caller's signal that something \
         else published for this pair at this stamp"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_constraint_violation_is_permanent_not_transient() {
    let (_pg, pool) = start_pg().await;
    let fv = composed();

    // 0011's canonical-shape CHECK refuses this id. The estimator must never
    // retry it: the same row will be refused by the same schema forever.
    let err = publish(&pool, 1_700_000_400, "not a product id", &fv, STALE)
        .await
        .expect_err("a non-canonical product id must be refused");
    assert!(
        !err.retryable(),
        "a CHECK violation classified as retryable is the silent-stall defect: {err:?}"
    );
    assert_eq!(err.class(), "permanent");

    // And 0014's bounds CHECK, from the other direction: an inverted bound
    // pair is a misconfiguration, not a transient.
    let inverted = LegStaleness {
        tape: Duration::from_secs(300),
        reference: Duration::from_secs(5),
    };
    let err = publish(&pool, 1_700_000_401, "EUR-USD", &fv, inverted)
        .await
        .expect_err("inverted staleness bounds must be refused");
    assert!(
        !err.retryable(),
        "an inverted bound pair is permanent: {err:?}"
    );

    // The other half of that CHECK, and the one an ordering-only constraint
    // would have let through: a zero bound ages every source of its class out
    // instantly, `FairValueConfig::validate` rejects it, and `0 >= 0` satisfies
    // an ordering test. Both halves of the validator have to be restated or the
    // database accepts a configuration the engine would have refused.
    let zeroed = LegStaleness {
        tape: Duration::ZERO,
        reference: Duration::ZERO,
    };
    let err = publish(&pool, 1_700_000_402, "EUR-USD", &fv, zeroed)
        .await
        .expect_err("a zero staleness bound must be refused");
    assert!(!err.retryable(), "a zero bound is permanent: {err:?}");
}

/// A failure that never reached a database is TRANSIENT.
///
/// The counterpart to the test above, and the one that matters most: every other
/// error case here is a CHECK violation, so without this the `Transient` variant
/// is never constructed, `"transient"` is never asserted, and **a `classify` that
/// returned `Permanent` unconditionally would pass the entire suite** — silently
/// collapsing the split back to the single-class behavior it exists to replace.
///
/// Closing the pool is the cheapest way to reach the non-`Database` arm with no
/// fake and no trait impl: the pool is gone, so the row cannot reach a server
/// that could judge it.
///
/// Asserts through `retryable()` / `class()` rather than on the variant, because
/// which non-`Database` error a closed pool yields (`PoolClosed` against
/// `PoolTimedOut`) is sqlx's choice and not this module's contract.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_failure_that_never_reached_a_database_is_transient() {
    let (_pg, pool) = start_pg().await;
    let fv = composed();

    // Prove the pool worked first, so a failure below is the close and not a
    // broken fixture.
    publish(&pool, 1_700_000_500, "EUR-USD", &fv, STALE)
        .await
        .expect("the pool must work before it is closed");

    pool.close().await;

    let err = publish(&pool, 1_700_000_501, "EUR-USD", &fv, STALE)
        .await
        .expect_err("publishing through a closed pool must fail");
    assert!(
        err.retryable(),
        "a failure that never reached a database must be retryable: {err:?}"
    );
    assert_eq!(err.class(), "transient");
}
