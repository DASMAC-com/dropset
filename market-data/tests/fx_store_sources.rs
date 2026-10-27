//! The store reader's roster is the **caller's**, not the reader's.
//!
//! This pins the one behavior the constructor split exists to provide, and it
//! needs a test because the old shape failed *silently*: with the FX venue list
//! hardcoded inside `FxStoreSource::new`, asking for a crypto source returned
//! an empty result rather than an error. The rows were in `cex_prices` the whole
//! time, under sources the reader would not ask for — so a consumer composing
//! `fair = fx x basis` from the store could reach exactly one of its three legs
//! and the other two looked like collectors that had not caught up yet.
//!
//! That is the failure mode worth a test: not a wrong value, but a missing leg
//! indistinguishable from a slow one.
//!
//! Needs a Docker daemon, so `#[ignore]`d like the fence tests:
//!
//! ```sh
//! cargo test -p dropset-market-data -- --ignored
//! ```
//!
//! **A merge gate** — the Tests (Postgres) job's `--run-ignored all`
//! invocation selects `dropset-market-data`, so this runs in the merge queue.
//! It is still skipped by a bare local `cargo test`, so run the command above
//! before pushing a change that touches it.

mod common;

use common::{insert_bucket, start_pg};
use dropset_market_data::fx_store::{FxStoreSource, FX_STORE_SOURCES};

/// Sources spanning both sides of the old hardcoded boundary.
const FX_VENUE: &str = "oanda";
const CRYPTO_VENUE: &str = "coinbase";

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn an_explicit_source_set_reaches_a_non_fx_venue() {
    let (_pg, pool) = start_pg().await;

    insert_bucket(&pool, CRYPTO_VENUE, "EURC-USDC", 1_700_000_060, 1.141).await;
    insert_bucket(&pool, "kraken", "USDC-USD", 1_700_000_060, 1.0).await;

    let rows = FxStoreSource::new(
        "store:crypto",
        pool.clone(),
        vec![CRYPTO_VENUE.to_string(), "kraken".to_string()],
        vec!["EURC-USDC".to_string(), "USDC-USD".to_string()],
    )
    .latest()
    .await
    .expect("read the crypto legs");

    assert_eq!(rows.len(), 2, "both crypto-side series must come back");
    let eurc = rows
        .iter()
        .find(|r| r.product_id == "EURC-USDC")
        .expect("the basis leg's series");
    assert_eq!(eurc.source, CRYPTO_VENUE);
    assert_eq!(eurc.close, 1.141);
    // The reader projects the bucket CLOSE, not its start.
    assert_eq!(
        eurc.published_at, 1_700_000_060,
        "published_at is the close"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_fx_constructor_asks_only_for_the_fx_roster() {
    let (_pg, pool) = start_pg().await;

    // Same pair id written by two venues, one on each side of the FX roster.
    insert_bucket(&pool, FX_VENUE, "EUR-USD", 1_700_000_060, 1.14).await;
    insert_bucket(&pool, CRYPTO_VENUE, "EUR-USD", 1_700_000_120, 1.99).await;

    let rows = FxStoreSource::fx("store:fx", pool.clone(), vec!["EUR-USD".to_string()])
        .latest()
        .await
        .expect("read the FX leg");

    // The crypto row is newer, so a reader that ignored its source list would
    // return it — and the maker would price its FX anchor off a venue that was
    // never designated for that leg.
    assert_eq!(rows.len(), 1, "only the FX-rostered venue answers");
    assert_eq!(rows[0].source, FX_VENUE);
    assert_eq!(rows[0].close, 1.14);
    assert!(
        FX_STORE_SOURCES.contains(&FX_VENUE),
        "the roster this asserts against is the designated one"
    );
    assert!(
        !FX_STORE_SOURCES.contains(&CRYPTO_VENUE),
        "a crypto venue must not be on the FX anchor roster, or this test proves nothing"
    );
}

/// A series holding two bucket widths is ranked by the projected close, not by
/// `bucket_start`. The statement walks each width separately to stay on the
/// primary key, so this pins that the walk still compares across widths, in
/// both directions: a daily bucket that opened long before the newest minute
/// bucket but closed after it is the newer print, and a daily bucket that
/// closed first loses to the minute bucket.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_newest_close_wins_across_bucket_widths() {
    let (_pg, pool) = start_pg().await;
    const CLOSE: i64 = 1_700_086_400;

    let insert_daily = |product_id: &'static str, published_at: i64, close: f64| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO cex_prices
                     (source, product_id, granularity_secs, bucket_start,
                      low, high, open, close, volume)
                 VALUES ($1, $2, 86400, $3, $4, $4, $4, $4, 0)",
            )
            .bind(FX_VENUE)
            .bind(product_id)
            .bind(published_at - 86_400)
            .bind(close)
            .execute(&pool)
            .await
            .expect("insert the daily bucket");
        }
    };

    // EUR-USD: the daily bucket closes last, so the coarser width wins.
    insert_bucket(&pool, FX_VENUE, "EUR-USD", CLOSE - 1_200, 1.10).await;
    insert_bucket(&pool, FX_VENUE, "EUR-USD", CLOSE - 600, 1.11).await;
    insert_daily("EUR-USD", CLOSE, 1.12).await;
    // GBP-USD, the mirror: the minute bucket closes last, so the finer width
    // wins — which a reader preferring the widest granularity would get wrong.
    insert_bucket(&pool, FX_VENUE, "GBP-USD", CLOSE, 1.31).await;
    insert_daily("GBP-USD", CLOSE - 600, 1.30).await;

    // EUR-USD asked for twice: the walk visits it twice, and the result must
    // still carry one row for it.
    let products = ["EUR-USD", "GBP-USD", "EUR-USD"];
    let mut rows = FxStoreSource::fx(
        "store:fx",
        pool.clone(),
        products.iter().map(|p| p.to_string()).collect(),
    )
    .latest()
    .await
    .expect("read the FX legs");
    rows.sort_by(|a, b| a.product_id.cmp(&b.product_id));

    assert_eq!(
        rows.len(),
        2,
        "one row per series, whatever widths it holds"
    );
    assert_eq!(rows[0].product_id, "EUR-USD");
    assert_eq!(rows[0].close, 1.12, "the daily bucket closed last");
    assert_eq!(rows[0].published_at, CLOSE);
    assert_eq!(rows[1].product_id, "GBP-USD");
    assert_eq!(rows[1].close, 1.31, "the minute bucket closed last");
    assert_eq!(rows[1].published_at, CLOSE);
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn an_empty_result_is_not_an_error() {
    let (_pg, pool) = start_pg().await;

    // Nothing written. The reader must distinguish "read succeeded, no rows"
    // from "read failed" — its `Source` impl always emits, and the consumer's
    // store-silence guard keys off the difference.
    let rows = FxStoreSource::fx("store:fx", pool.clone(), vec!["EUR-USD".to_string()])
        .latest()
        .await
        .expect("an empty store is a successful read");
    assert!(rows.is_empty());
}
