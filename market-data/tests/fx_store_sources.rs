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
//! **Operator-run, not a merge gate** — CI's only `--run-ignored` invocation
//! selects two other crates, so nothing here executes in the merge queue. The
//! wiring is tracked separately.

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
