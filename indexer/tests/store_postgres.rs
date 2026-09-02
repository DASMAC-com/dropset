//! End-to-end tests for the indexer's Postgres path against a real database
//! in a throwaway container: the store sink's decode-and-write, its
//! idempotency under the at-least-once contract, and one aggregation pass
//! folding written legs into takes and market rollups.
//!
//! These drive the framework seams the indexer now runs on
//! (docs/data-feeds.md §6) rather than the store's methods directly, so what
//! is covered is the path the worker actually takes: a `RawTx` carrying
//! tagged event blobs goes through `EventWriter` inside a `StoreSink`, and
//! the aggregator reads what that committed.
//!
//! The schema is provisioned through `dropset-db-schema`, the single schema
//! owner — nothing here issues DDL of its own, matching how a deployment is
//! migrated.
//!
//! These need a Docker daemon, so they are `#[ignore]`d and skipped by the
//! default test run. Run them with:
//!
//! ```sh
//! cargo test -p dropset-indexer -- --ignored
//! ```

use dropset_feeds::{
    connect, Batch, Cursor, CursorStore, InnerIx, PgCursorStore, RawTx, Sink, StoreSink,
};
use dropset_indexer::aggregate::AggregateSink;
use dropset_indexer::store::{EventWriter, Store};
use dropset_sdk::events::{DEPOSIT_EVENT_DISCRIMINATOR, EVENT_IX_TAG_LE, FILL_EVENT_DISCRIMINATOR};
use dropset_sdk::types::{DepositEvent, FillEvent};
use dropset_sdk::DROPSET_ID;
use rust_decimal::Decimal;
use solana_pubkey::Pubkey;
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};

/// Start a throwaway Postgres, migrate it, and connect both the indexer's
/// store and a framework pool over it.
async fn start_pg() -> (ContainerAsync<Postgres>, PgPool, Store) {
    let container = Postgres::default()
        .start()
        .await
        .expect("start postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("resolve mapped port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = connect(&url).await.expect("connect pool");
    dropset_db_schema::migrate(&pool)
        .await
        .expect("apply shared schema");
    // `Store::connect` asserts the schema rather than creating it, so this
    // also covers the version fence the worker hits at startup.
    let store = Store::connect(&url).await.expect("connect store");
    (container, pool, store)
}

/// Wrap an event body in the `[tag][discriminator][borsh body]` envelope an
/// `emit_cpi!` inner instruction carries.
fn tagged(discriminator: [u8; 8], body: &impl borsh::BorshSerialize) -> Vec<u8> {
    let mut data = EVENT_IX_TAG_LE.to_vec();
    data.extend_from_slice(&discriminator);
    borsh::to_writer(&mut data, body).expect("serialize event body");
    data
}

/// One fill leg on `market`, priced so the take totals are easy to read.
fn fill(market: Pubkey, base: u64, quote: u64, fee: u64) -> Vec<u8> {
    tagged(
        FILL_EVENT_DISCRIMINATOR,
        &FillEvent {
            market,
            taker: Pubkey::new_unique(),
            leader: Pubkey::new_unique(),
            quote_authority: Pubkey::new_unique(),
            side: 0,
            pad: [0; 7],
            sector_idx: 1,
            level_idx: 2,
            fill_base: base,
            fill_quote: quote,
            fill_price: 1,
            pad2: [0; 4],
            base_atoms_after: 0,
            quote_atoms_after: 0,
            nonce_after: 0,
            taker_fee_atoms: fee,
        },
    )
}

/// A non-fill event, which lands in the JSONB fidelity table rather than the
/// typed fills table.
fn deposit(market: Pubkey) -> Vec<u8> {
    tagged(
        DEPOSIT_EVENT_DISCRIMINATOR,
        &DepositEvent {
            market,
            sector_idx: 1,
            depositor: Pubkey::new_unique(),
            is_leader: true,
            is_seeding: false,
            base_in: 10,
            quote_in: 20,
            shares_out: 30,
            total_shares_after: 30,
            leader_shares_after: 30,
            base_atoms_after: 10,
            quote_atoms_after: 20,
        },
    )
}

/// The program these tests index. Placed at account-key index 0, so every
/// blob built by [`raw_tx`] is attributed to it and passes the
/// emitting-program check in `decode_tx`.
const INDEXED_PROGRAM: Pubkey = DROPSET_ID;

/// A transaction whose blobs all claim to come from [`INDEXED_PROGRAM`].
/// Use [`raw_tx_from`] to attribute a blob elsewhere.
fn raw_tx(slot: i64, signature: &str, blobs: Vec<Vec<u8>>) -> RawTx {
    raw_tx_from(
        slot,
        signature,
        blobs.into_iter().map(|b| (0u8, b)).collect(),
    )
}

/// A transaction whose blobs carry the given `program_id_index` each.
/// Index 0 is [`INDEXED_PROGRAM`]; index 1 is a foreign program sharing
/// the transaction.
fn raw_tx_from(slot: i64, signature: &str, blobs: Vec<(u8, Vec<u8>)>) -> RawTx {
    RawTx {
        slot,
        txn_index: 0,
        signature: signature.to_string(),
        block_time: Some(1_700_000_000),
        inner_ix_blobs: blobs
            .into_iter()
            .map(|(program_id_index, data)| InnerIx {
                program_id_index,
                data,
            })
            .collect(),
        static_account_keys: vec![INDEXED_PROGRAM, Pubkey::new_from_array([9; 32])],
        loaded_writable: Vec::new(),
        loaded_readonly: Vec::new(),
    }
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .expect("count rows")
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn store_sink_routes_events_to_their_tables_and_advances_the_cursor() {
    let (_pg, pool, _store) = start_pg().await;
    let market = Pubkey::new_unique();
    let feed = "rpc:test";
    let mut sink = StoreSink::new(pool.clone(), feed, EventWriter::new(INDEXED_PROGRAM));

    // One transaction carrying a fill, a deposit, and a blob that is not a
    // Dropset event at all — the last must be ignored rather than stored.
    let cursor = Cursor::from_json(serde_json::json!({ "last_signature": "sig-a" }));
    let batch = Batch::new(vec![raw_tx(
        10,
        "sig-a",
        vec![vec![0xaa, 0xbb], fill(market, 100, 200, 3), deposit(market)],
    )])
    .with_cursor(cursor.clone());

    sink.handle(&batch).await.unwrap();

    assert_eq!(count(&pool, "fill_events").await, 1);
    assert_eq!(count(&pool, "events").await, 1);
    // The framework saves the resume cursor after the batch commits.
    let cursors = PgCursorStore::new(pool.clone());
    assert_eq!(cursors.load(feed).await.unwrap(), Some(cursor));
}

/// The fabricated-fill path, end to end against a real database: a
/// well-formed `FillEvent` naming a real market, emitted by a **foreign**
/// program in a transaction that merely references ours, must write no
/// rows at all.
///
/// This is the whole attack. `getSignaturesForAddress` is address-indexed,
/// so such a transaction is polled and hydrated like any other, and both
/// halves of the `[tag][discriminator]` envelope are public — so nothing
/// but the emitting program id distinguishes this blob from a real fill.
/// Were it accepted it would flow into `fill_events`, then through the
/// take fold into `market_stats.volume_base/quote` and `last_price`, and
/// out onto the public `/v1` surface. There is no schema-level backstop:
/// the migrations carry no foreign key, so an unknown market inserts
/// freely.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_fill_forged_by_a_foreign_program_writes_no_rows() {
    let (_pg, pool, _store) = start_pg().await;
    let market = Pubkey::new_unique();
    let mut sink = StoreSink::new(pool.clone(), "rpc:test", EventWriter::new(INDEXED_PROGRAM));

    // Index 1 is the foreign program; the payload is otherwise genuine.
    let batch = Batch::new(vec![raw_tx_from(
        10,
        "sig-forged",
        vec![(1, fill(market, 100, 200, 3)), (1, deposit(market))],
    )]);

    sink.handle(&batch).await.unwrap();

    assert_eq!(count(&pool, "fill_events").await, 0, "forged fill stored");
    assert_eq!(count(&pool, "events").await, 0, "forged event stored");
}

/// The other half of the same transaction shape: our genuine event is
/// still indexed when a forgery rides alongside it. A check that dropped
/// the whole transaction on sight of one foreign blob would lose real
/// fills, so this pins the per-blob granularity.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_genuine_fill_survives_a_forgery_in_the_same_transaction() {
    let (_pg, pool, _store) = start_pg().await;
    let market = Pubkey::new_unique();
    let mut sink = StoreSink::new(pool.clone(), "rpc:test", EventWriter::new(INDEXED_PROGRAM));

    let batch = Batch::new(vec![raw_tx_from(
        11,
        "sig-mixed",
        vec![
            (1, fill(market, 999, 999, 9)),
            (0, fill(market, 100, 200, 3)),
        ],
    )]);

    sink.handle(&batch).await.unwrap();

    assert_eq!(count(&pool, "fill_events").await, 1);
    let base: Decimal = sqlx::query_scalar("SELECT fill_base FROM fill_events")
        .fetch_one(&pool)
        .await
        .expect("read the stored leg");
    assert_eq!(
        base,
        Decimal::from(100),
        "the genuine leg is the stored one"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn re_delivering_a_batch_writes_no_duplicate_rows() {
    let (_pg, pool, _store) = start_pg().await;
    let market = Pubkey::new_unique();
    let mut sink = StoreSink::new(pool.clone(), "rpc:test", EventWriter::new(INDEXED_PROGRAM));

    // Two legs of one take, so the event PK has to separate them by ordinal,
    // plus a non-fill so the JSONB table's ON CONFLICT is re-delivered too.
    let batch = Batch::new(vec![raw_tx(
        11,
        "sig-b",
        vec![
            fill(market, 100, 200, 3),
            fill(market, 50, 100, 1),
            deposit(market),
        ],
    )]);

    sink.handle(&batch).await.unwrap();
    assert_eq!(count(&pool, "fill_events").await, 2);
    assert_eq!(count(&pool, "events").await, 1);

    // A crash between the commit and the cursor save re-delivers the batch;
    // the writer's ON CONFLICT absorbs it (docs/data-feeds.md §3).
    sink.handle(&batch).await.unwrap();
    assert_eq!(count(&pool, "fill_events").await, 2);
    assert_eq!(count(&pool, "events").await, 1);
}

/// Sink order is load-bearing, not stylistic: the aggregator folds legs the
/// store sink has already committed, so running it first folds nothing. The
/// binary wires them in this order and a comment says why — this is what
/// fails if someone swaps them.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn folding_before_the_store_sink_commits_finds_nothing_to_fold() {
    let (_pg, pool, store) = start_pg().await;
    let market = Pubkey::new_unique();
    let mut store_sink =
        StoreSink::new(pool.clone(), "rpc:test", EventWriter::new(INDEXED_PROGRAM));
    let mut aggregate_sink = AggregateSink::new(store.clone(), 100);

    let batch = Batch::new(vec![raw_tx(13, "sig-e", vec![fill(market, 10, 20, 1)])]);

    // The wrong order: nothing is committed yet, so there is nothing to fold.
    aggregate_sink.handle(&batch).await.unwrap();
    assert!(store.list_takes(None, 10).await.unwrap().is_empty());

    // The runner's order: persist, then fold.
    store_sink.handle(&batch).await.unwrap();
    aggregate_sink.handle(&batch).await.unwrap();
    assert_eq!(store.list_takes(None, 10).await.unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_aggregate_sink_folds_written_legs_into_takes_and_rollups() {
    let (_pg, pool, store) = start_pg().await;
    let market = Pubkey::new_unique();
    let mut store_sink =
        StoreSink::new(pool.clone(), "rpc:test", EventWriter::new(INDEXED_PROGRAM));
    let mut aggregate_sink = AggregateSink::new(store.clone(), 100);

    let batch = Batch::new(vec![raw_tx(
        12,
        "sig-c",
        vec![fill(market, 100, 200, 3), fill(market, 50, 100, 1)],
    )]);

    // Sink order is the runner's: persist, then fold what was persisted.
    store_sink.handle(&batch).await.unwrap();
    aggregate_sink.handle(&batch).await.unwrap();

    // The two legs group into one take, summed.
    let takes = store.list_takes(None, 10).await.unwrap();
    assert_eq!(takes.len(), 1);
    assert_eq!(takes[0].signature, "sig-c");
    assert_eq!(takes[0].leg_count, 2);
    assert_eq!(takes[0].total_fill_base, Decimal::from(150));
    assert_eq!(takes[0].total_fill_quote, Decimal::from(300));
    assert_eq!(takes[0].total_taker_fee, Decimal::from(4));

    let markets = store.list_markets().await.unwrap();
    assert_eq!(markets.len(), 1);
    assert_eq!(markets[0].market, market.to_string());

    // Folding again is idempotent: the watermark has advanced past the legs,
    // and re-grouping a take recomputes the same row either way.
    aggregate_sink.handle(&batch).await.unwrap();
    assert_eq!(store.list_takes(None, 10).await.unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_aggregator_watermark_round_trips() {
    let (_pg, _pool, store) = start_pg().await;

    // A database that has never aggregated reports the zero watermark.
    let initial = store.cursor().await.unwrap();
    assert_eq!(initial.last_slot, 0);
    assert_eq!(initial.last_signature, "");

    store
        .set_cursor(&dropset_indexer::store::Cursor {
            last_slot: 42,
            last_txn_index: 1,
            last_event_ordinal: 2,
            last_signature: "sig-d".into(),
        })
        .await
        .unwrap();

    let reloaded = store.cursor().await.unwrap();
    assert_eq!(reloaded.last_slot, 42);
    assert_eq!(reloaded.last_txn_index, 1);
    assert_eq!(reloaded.last_event_ordinal, 2);
    assert_eq!(reloaded.last_signature, "sig-d");
}
