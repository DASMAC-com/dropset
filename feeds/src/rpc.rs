//! The RPC-poll transport (`rpc` feature): poll `getSignaturesForAddress` +
//! `getTransaction` at `finalized`, generalized over program id. Extracted
//! from `indexer/src/ingest.rs` (docs/data-feeds.md §2, §4).

// cspell:word nonblocking

use crate::backfill::{Backfill, Step};
use crate::cursor::Cursor;
use crate::record::Batch;
use crate::source::Source;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_client::rpc_response::RpcConfirmedTransactionStatusWithSignature;
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, UiInstruction, UiTransactionEncoding,
};
use std::str::FromStr;

/// How many signature pages one [`Source::next`] enumerates before yielding.
/// This bounds the work a single poll may do, not the depth of the walk: an
/// unfinished walk resumes from where it stopped on the following poll, so a
/// deep backlog is enumerated across several polls rather than one long one.
const DEFAULT_MAX_PAGES_PER_POLL: usize = 16;

/// One page of the backward walk: signature summaries, newest-first, as the
/// RPC returns them.
type SigPage = Vec<RpcConfirmedTransactionStatusWithSignature>;

/// The single RPC call the backward walk makes, behind a seam so the walk's
/// paging — the `before` chaining, the saturation test, the page cap — is
/// unit-testable without a validator. The production implementation is the
/// one below, on the real client.
#[async_trait]
trait SignaturePager: Send + Sync {
    async fn page(
        &self,
        program_id: &Pubkey,
        config: GetConfirmedSignaturesForAddress2Config,
    ) -> Result<SigPage>;
}

#[async_trait]
impl SignaturePager for RpcClient {
    async fn page(
        &self,
        program_id: &Pubkey,
        config: GetConfirmedSignaturesForAddress2Config,
    ) -> Result<SigPage> {
        Ok(self
            .get_signatures_for_address_with_config(program_id, config)
            .await?)
    }
}

/// One segment of the backward walk: what it found, where to resume, and
/// whether it got to the bound.
struct WalkSegment {
    /// The `before` marker each discovered page was fetched with, in walk
    /// order (newest page first). `None` is the first page — the present.
    markers: Vec<Option<Signature>>,
    /// The `before` to continue from, if the walk stopped at `max_pages`.
    resume: Option<Signature>,
    /// Whether the walk reached the resume cursor (or the start of history).
    reached_bound: bool,
}

/// Enumerate backwards from `start_before` toward `until`, at most
/// `max_pages` pages.
///
/// Signature summaries only — no transaction is fetched — and only each
/// page's `before` marker is kept, so enumerating a deep backlog costs one
/// signature per page rather than per record. That is what lets the walk run
/// all the way to the bound, which [`Backfill`] requires before anything may
/// be emitted.
async fn enumerate_pages<P: SignaturePager + ?Sized>(
    client: &P,
    program_id: &Pubkey,
    until: Option<Signature>,
    start_before: Option<Signature>,
    batch_limit: usize,
    max_pages: usize,
) -> Result<WalkSegment> {
    let mut markers = Vec::new();
    let mut before = start_before;
    for _ in 0..max_pages {
        let config = GetConfirmedSignaturesForAddress2Config {
            before,
            until,
            limit: Some(batch_limit),
            commitment: Some(CommitmentConfig::finalized()),
        };
        let page = client.page(program_id, config).await?;
        if page.is_empty() {
            return Ok(WalkSegment {
                markers,
                resume: before,
                reached_bound: true,
            });
        }
        markers.push(before);
        // A short page means the window between `before` and the cursor is
        // exhausted: the walk has reached the bound (or the start of the
        // program's history).
        let saturated = page.len() >= batch_limit;
        before = Some(Signature::from_str(&page[page.len() - 1].signature)?);
        if !saturated {
            return Ok(WalkSegment {
                markers,
                resume: before,
                reached_bound: true,
            });
        }
    }
    Ok(WalkSegment {
        markers,
        resume: before,
        reached_bound: false,
    })
}

/// A decoded transaction touching the watched program: its coordinates plus
/// the flattened, base58-decoded inner-instruction `data` blobs. Consumers run
/// their own event decoder over `inner_ix_blobs`; the framework does not decode
/// (docs/data-feeds.md §4). The framework twin of the indexer's `RawTx`.
#[derive(Clone, Debug)]
pub struct RawTx {
    pub slot: i64,
    /// The RPC path can't cheaply learn a tx's position in its block; the
    /// signature already makes a consumer's PK unique, so this is a `0` filler
    /// (a geyser path would supply the true index).
    pub txn_index: i64,
    pub signature: String,
    pub block_time: Option<i64>,
    pub inner_ix_blobs: Vec<Vec<u8>>,
}

/// The opaque cursor an RPC poll persists: the newest signature already
/// returned, used as the `until` bound on the next poll.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct RpcCursor {
    last_signature: String,
}

/// A poll source over `getSignaturesForAddress` + `getTransaction` at
/// `finalized`, generalized over program id.
///
/// **Backfill windowing.** `getSignaturesForAddress` answers newest-first
/// and capped at `batch_limit`, so a backlog deeper than one page cannot be
/// drained by a single call, and the resume cursor is an exclusive *lower*
/// bound — advancing it mid-backlog discards everything older rather than
/// deferring it. The source therefore runs in two phases behind a
/// [`Backfill`] pager: it *enumerates* backwards with `before`, keeping only
/// each page's marker, until it reaches the resume cursor; then it
/// *hydrates* one page per [`Source::next`], oldest page first, advancing
/// the cursor to each emitted page's newest signature. Because emission runs
/// oldest → newest, nothing is ever left behind the cursor.
///
/// Enumeration is bounded per poll, not in total: a walk that hits
/// [`DEFAULT_MAX_PAGES_PER_POLL`] resumes where it stopped on the next poll
/// (reporting a backlog, so the runner loops straight back) rather than
/// emitting from an unknown position.
pub struct RpcPollSource {
    name: String,
    client: RpcClient,
    program_id: Pubkey,
    batch_limit: usize,
    max_pages_per_poll: usize,
    /// The newest signature already emitted; the `until` bound on every
    /// request until it moves.
    last_signature: Option<Signature>,
    /// Where an unfinished backward walk resumes from.
    walk_before: Option<Signature>,
    /// Markers for pages enumerated but not yet emitted, drained
    /// oldest-first.
    pager: Backfill<Option<Signature>>,
}

impl RpcPollSource {
    /// A source polling `program_id` at `rpc_url`, up to `batch_limit`
    /// signatures per page. Starts from the present; use [`Self::resume_from`]
    /// to continue from a saved cursor.
    pub fn new(rpc_url: String, program_id: Pubkey, batch_limit: usize) -> Self {
        Self {
            name: format!("rpc:{program_id}"),
            client: RpcClient::new_with_commitment(rpc_url, CommitmentConfig::finalized()),
            program_id,
            // A zero page would make every page look saturated and the walk
            // never reach the bound; one signature per page is the floor.
            batch_limit: batch_limit.max(1),
            max_pages_per_poll: DEFAULT_MAX_PAGES_PER_POLL,
            last_signature: None,
            walk_before: None,
            pager: Backfill::new(),
        }
    }

    /// Cap how many signature pages one poll enumerates before yielding.
    /// Raising it reaches the bound in fewer polls at the cost of a longer
    /// single `next()`; the default is [`DEFAULT_MAX_PAGES_PER_POLL`].
    pub fn with_max_pages_per_poll(mut self, pages: usize) -> Self {
        self.max_pages_per_poll = pages.max(1);
        self
    }

    /// Resume from a cursor loaded from the store at startup.
    pub fn resume_from(mut self, cursor: &Cursor) -> Result<Self> {
        let c: RpcCursor = cursor.get()?;
        self.last_signature = Some(Signature::from_str(&c.last_signature)?);
        Ok(self)
    }

    /// Re-request one enumerated page from its `before` marker. The current
    /// resume cursor bounds it from below, so the window is exactly the page
    /// the walk saw — a page that has since grown is still capped at
    /// `batch_limit`, and anything that falls outside is picked up by the
    /// walk that follows.
    async fn fetch_page(&self, before: Option<Signature>) -> Result<SigPage> {
        let config = GetConfirmedSignaturesForAddress2Config {
            before,
            until: self.last_signature,
            limit: Some(self.batch_limit),
            commitment: Some(CommitmentConfig::finalized()),
        };
        self.client.page(&self.program_id, config).await
    }

    /// Fetch each transaction in `page` and flatten it, oldest-first.
    /// Transactions that failed on-chain carry no events worth storing and
    /// are skipped.
    async fn hydrate(&self, page: &SigPage) -> Result<Vec<RawTx>> {
        let tx_config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Base64),
            commitment: Some(CommitmentConfig::finalized()),
            max_supported_transaction_version: Some(0),
        };
        let mut out = Vec::new();
        // The page is newest-first; consumers want oldest-first.
        for s in page.iter().rev() {
            if s.err.is_some() {
                continue;
            }
            let signature = Signature::from_str(&s.signature)?;
            let tx = self
                .client
                .get_transaction_with_config(&signature, tx_config)
                .await?;
            if let Some(raw) = to_raw_tx(&s.signature, tx) {
                out.push(raw);
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl Source for RpcPollSource {
    type Record = RawTx;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<RawTx>> {
        loop {
            match self.pager.step() {
                Step::Enumerate => {
                    let segment = enumerate_pages(
                        &self.client,
                        &self.program_id,
                        self.last_signature,
                        self.walk_before,
                        self.batch_limit,
                        self.max_pages_per_poll,
                    )
                    .await?;
                    self.walk_before = segment.resume;
                    self.pager.extend(segment.markers, segment.reached_bound);
                    if !segment.reached_bound {
                        // Still enumerating, so nothing may be emitted yet.
                        // Report a backlog and let the runner loop straight
                        // back rather than sleeping mid-walk.
                        return Ok(Batch::new(vec![]).with_caught_up(false));
                    }
                }
                Step::Emit(before) => {
                    let page = self.fetch_page(before).await?;
                    if page.is_empty() {
                        // The window emptied between enumeration and here —
                        // nothing to emit, and the cursor must not move.
                        continue;
                    }
                    // The page's newest signature becomes the cursor. Taking
                    // it before hydration is safe: the batch is only durable
                    // once the sink commits it, and a crash in between
                    // re-fetches this page (the at-least-once contract,
                    // docs/data-feeds.md §3).
                    let newest = Signature::from_str(&page[0].signature)?;
                    let out = self.hydrate(&page).await?;

                    self.last_signature = Some(newest);
                    let cursor = Cursor::new(&RpcCursor {
                        last_signature: newest.to_string(),
                    })?;
                    return Ok(Batch::new(out)
                        .with_cursor(cursor)
                        .with_caught_up(self.pager.caught_up()));
                }
                Step::Done => {
                    // Backlog drained; the next poll walks from the present.
                    self.pager.restart();
                    self.walk_before = None;
                    return Ok(Batch::new(vec![]));
                }
            }
        }
    }
}

/// Flatten a fetched transaction's inner instructions into ordered,
/// base58-decoded `data` blobs.
fn to_raw_tx(signature: &str, tx: EncodedConfirmedTransactionWithStatusMeta) -> Option<RawTx> {
    let slot = tx.slot as i64;
    let block_time = tx.block_time;
    let meta = tx.transaction.meta?;
    let mut inner_ix_blobs = Vec::new();
    if let OptionSerializer::Some(groups) = meta.inner_instructions {
        for group in groups {
            for ix in group.instructions {
                if let UiInstruction::Compiled(c) = ix {
                    if let Ok(bytes) = bs58::decode(&c.data).into_vec() {
                        inner_ix_blobs.push(bytes);
                    }
                }
            }
        }
    }
    Some(RawTx {
        slot,
        txn_index: 0,
        signature: signature.to_string(),
        block_time,
        inner_ix_blobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use solana_transaction_status::{
        EncodedTransaction, EncodedTransactionWithStatusMeta, UiTransactionStatusMeta,
    };

    /// A fetched transaction carrying `meta`, whose `innerInstructions` come
    /// from the JSON `getTransaction` returns.
    ///
    /// Only the meta is deserialized from wire JSON: that is the shape this
    /// adapter parses, and building it from JSON keeps the fixture readable
    /// and tolerant of unrelated field additions upstream. The transaction
    /// body is built directly as the `LegacyBinary` variant — the adapter
    /// never reads it, and skipping the untagged `EncodedTransaction` parse
    /// avoids a whole message fixture that would prove nothing.
    fn encoded_tx(
        slot: u64,
        block_time: Option<i64>,
        inner: serde_json::Value,
    ) -> EncodedConfirmedTransactionWithStatusMeta {
        let meta: UiTransactionStatusMeta = serde_json::from_value(json!({
            "err": null,
            "status": { "Ok": null },
            "fee": 5000,
            "preBalances": [],
            "postBalances": [],
            "innerInstructions": inner,
            "logMessages": [],
            "preTokenBalances": [],
            "postTokenBalances": [],
            "rewards": [],
        }))
        .expect("fixture matches the transaction-meta wire shape");
        with_meta(slot, block_time, Some(meta))
    }

    /// A fetched transaction with the given (possibly absent) meta.
    fn with_meta(
        slot: u64,
        block_time: Option<i64>,
        meta: Option<UiTransactionStatusMeta>,
    ) -> EncodedConfirmedTransactionWithStatusMeta {
        EncodedConfirmedTransactionWithStatusMeta {
            slot,
            transaction: EncodedTransactionWithStatusMeta {
                transaction: EncodedTransaction::LegacyBinary(String::new()),
                meta,
                version: None,
            },
            block_time,
        }
    }

    /// One compiled inner instruction carrying base58 `data`.
    fn compiled_ix(data: &[u8]) -> serde_json::Value {
        json!({
            "programIdIndex": 4,
            "accounts": [],
            "data": bs58::encode(data).into_string(),
            "stackHeight": 2,
        })
    }

    #[test]
    fn flattens_inner_instructions_in_order_and_decodes_base58() {
        // Two groups, two instructions in the first — the flatten must keep
        // group order and within-group order, since a consumer's event
        // ordinal is the position in this list.
        let tx = encoded_tx(
            42,
            Some(1_700_000_000),
            json!([
                { "index": 0, "instructions": [compiled_ix(&[1, 2, 3]), compiled_ix(&[4])] },
                { "index": 1, "instructions": [compiled_ix(&[5, 6])] },
            ]),
        );

        let raw = to_raw_tx("sig-1", tx).expect("meta present");
        assert_eq!(raw.slot, 42);
        assert_eq!(raw.block_time, Some(1_700_000_000));
        assert_eq!(raw.signature, "sig-1");
        // The RPC path cannot learn a tx's index in its block.
        assert_eq!(raw.txn_index, 0);
        assert_eq!(raw.inner_ix_blobs, vec![vec![1, 2, 3], vec![4], vec![5, 6]]);
    }

    #[test]
    fn a_transaction_with_no_inner_instructions_yields_no_blobs() {
        let raw = to_raw_tx("sig-2", encoded_tx(7, None, json!([]))).expect("meta present");
        assert!(raw.inner_ix_blobs.is_empty());
        assert_eq!(raw.block_time, None);
    }

    /// `innerInstructions` is an `OptionSerializer` field: RPC nodes running
    /// without transaction-status indexing omit it entirely, which must read
    /// as "no blobs" rather than panic.
    #[test]
    fn an_absent_inner_instruction_field_yields_no_blobs() {
        let raw = to_raw_tx("sig-3", encoded_tx(9, None, json!(null))).expect("meta present");
        assert!(raw.inner_ix_blobs.is_empty());
    }

    /// A transaction fetched without meta cannot be decoded — the adapter
    /// drops it rather than emitting a record with no instruction data.
    #[test]
    fn a_transaction_without_meta_is_dropped() {
        assert!(to_raw_tx("sig-4", with_meta(1, None, None)).is_none());
    }

    /// A stand-in ledger: signatures newest-first, paged exactly as
    /// `getSignaturesForAddress` does — `before` excludes everything at or
    /// newer than it, `until` excludes everything at or older than it, and
    /// at most `limit` come back.
    struct FakeLedger {
        /// Every signature the program has, newest-first.
        signatures: Vec<String>,
    }

    #[async_trait]
    impl SignaturePager for FakeLedger {
        async fn page(
            &self,
            _program_id: &Pubkey,
            config: GetConfirmedSignaturesForAddress2Config,
        ) -> Result<SigPage> {
            let position = |sig: Signature| {
                let s = sig.to_string();
                self.signatures.iter().position(|have| *have == s)
            };
            let start = config.before.and_then(position).map_or(0, |i| i + 1);
            let end = config
                .until
                .and_then(position)
                .unwrap_or(self.signatures.len());
            let limit = config.limit.unwrap_or(self.signatures.len());
            Ok(self.signatures[start.min(end)..end]
                .iter()
                .take(limit)
                .map(|signature| RpcConfirmedTransactionStatusWithSignature {
                    signature: signature.clone(),
                    slot: 1,
                    err: None,
                    memo: None,
                    block_time: None,
                    confirmation_status: None,
                })
                .collect())
        }
    }

    /// `n` distinct valid signatures, newest-first — index 0 is the newest.
    fn ledger(n: usize) -> FakeLedger {
        FakeLedger {
            signatures: (0..n)
                .map(|i| {
                    let mut bytes = [0u8; 64];
                    bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
                    Signature::from(bytes).to_string()
                })
                .collect(),
        }
    }

    /// Drive the source's own `next()` logic against a fake ledger, without
    /// the transaction hydration that would need a validator: enumerate,
    /// then emit page by page, collecting every signature in emission order.
    /// This is the shape [`Source::next`] runs; keeping it in one place lets
    /// each test vary the page size and per-poll cap.
    async fn drain(
        led: &FakeLedger,
        batch_limit: usize,
        max_pages_per_poll: usize,
    ) -> (Vec<String>, usize) {
        let program = Pubkey::new_unique();
        let mut pager: Backfill<Option<Signature>> = Backfill::new();
        let mut cursor: Option<Signature> = None;
        let mut walk_before: Option<Signature> = None;
        let mut emitted = Vec::new();
        let mut polls = 0;

        // A generous bound: the loop must terminate on its own well inside it.
        for _ in 0..256 {
            match pager.step() {
                Step::Enumerate => {
                    let segment = enumerate_pages(
                        led,
                        &program,
                        cursor,
                        walk_before,
                        batch_limit,
                        max_pages_per_poll,
                    )
                    .await
                    .unwrap();
                    walk_before = segment.resume;
                    pager.extend(segment.markers, segment.reached_bound);
                    if !segment.reached_bound {
                        polls += 1;
                    }
                }
                Step::Emit(before) => {
                    let config = GetConfirmedSignaturesForAddress2Config {
                        before,
                        until: cursor,
                        limit: Some(batch_limit),
                        commitment: Some(CommitmentConfig::finalized()),
                    };
                    let page = led.page(&program, config).await.unwrap();
                    if page.is_empty() {
                        continue;
                    }
                    cursor = Some(Signature::from_str(&page[0].signature).unwrap());
                    emitted.extend(page.iter().rev().map(|s| s.signature.clone()));
                    polls += 1;
                }
                Step::Done => return (emitted, polls),
            }
        }
        panic!("the drive loop did not terminate");
    }

    /// Every signature the ledger holds, oldest-first — what a correct drain
    /// must produce.
    fn oldest_first(led: &FakeLedger) -> Vec<String> {
        led.signatures.iter().rev().cloned().collect()
    }

    /// A backlog that fits in one page needs one call and reaches the bound.
    #[tokio::test]
    async fn a_short_page_ends_the_walk_at_the_bound() {
        let led = ledger(3);
        let segment = enumerate_pages(&led, &Pubkey::new_unique(), None, None, 10, 16)
            .await
            .unwrap();

        assert!(segment.reached_bound);
        assert_eq!(segment.markers, vec![None]);
    }

    /// The walk chains `before` from each page's oldest signature, so the
    /// markers address consecutive, non-overlapping windows.
    #[tokio::test]
    async fn the_walk_chains_before_markers_across_pages() {
        let led = ledger(7);
        let segment = enumerate_pages(&led, &Pubkey::new_unique(), None, None, 3, 16)
            .await
            .unwrap();

        assert!(segment.reached_bound);
        // 3 + 3 + 1: the short last page is what ends the walk.
        let sig = |i: usize| Signature::from_str(&led.signatures[i]).unwrap();
        assert_eq!(segment.markers, vec![None, Some(sig(2)), Some(sig(5))]);
    }

    /// A walk stops at the resume cursor: signatures at or older than it were
    /// already emitted and must not be enumerated again.
    #[tokio::test]
    async fn the_walk_stops_at_the_resume_cursor() {
        let led = ledger(6);
        // Resume just below the three newest.
        let until = Signature::from_str(&led.signatures[3]).unwrap();
        let segment = enumerate_pages(&led, &Pubkey::new_unique(), Some(until), None, 2, 16)
            .await
            .unwrap();

        assert!(segment.reached_bound);
        let sig = |i: usize| Signature::from_str(&led.signatures[i]).unwrap();
        assert_eq!(segment.markers, vec![None, Some(sig(1))]);
    }

    /// Hitting the per-poll cap reports an unreached bound and where to
    /// resume, so the next poll continues the same walk instead of
    /// restarting or emitting from an unknown position.
    #[tokio::test]
    async fn hitting_the_per_poll_cap_reports_where_to_resume() {
        let led = ledger(100);
        let segment = enumerate_pages(&led, &Pubkey::new_unique(), None, None, 5, 3)
            .await
            .unwrap();

        assert!(!segment.reached_bound);
        assert_eq!(segment.markers.len(), 3);
        // Three pages of five consumed the fifteen newest signatures.
        assert_eq!(
            segment.resume,
            Some(Signature::from_str(&led.signatures[14]).unwrap())
        );
    }

    /// The regression this whole change exists for: a backlog deeper than one
    /// page is emitted in full, oldest-first, with nothing skipped. The old
    /// source took the newest page and jumped the cursor to its newest
    /// signature, losing every record below it.
    #[tokio::test]
    async fn a_deep_backlog_drains_completely_and_in_order() {
        let led = ledger(9);
        let (emitted, _) = drain(&led, 2, 16).await;
        assert_eq!(emitted, oldest_first(&led));
    }

    /// The same guarantee when the per-poll cap splits the walk across
    /// several polls: enumeration resumes where it stopped, and emission
    /// still begins at the true oldest record.
    #[tokio::test]
    async fn a_walk_split_across_polls_still_drains_in_order() {
        let led = ledger(9);
        // Page size 2 with a 2-page cap: the walk needs three segments.
        let (emitted, polls) = drain(&led, 2, 2).await;
        assert_eq!(emitted, oldest_first(&led));
        // Enumeration cost real polls, so the split is genuinely exercised.
        assert!(polls > 5, "expected a multi-poll drain, got {polls}");
    }

    /// A ledger with nothing newer than the cursor emits nothing and reports
    /// the source current.
    #[tokio::test]
    async fn an_empty_window_emits_nothing() {
        let led = ledger(0);
        let (emitted, _) = drain(&led, 5, 16).await;
        assert!(emitted.is_empty());
    }

    /// Undecodable base58 in one instruction skips that blob without
    /// discarding the rest of the transaction.
    #[test]
    fn undecodable_instruction_data_is_skipped() {
        let tx = encoded_tx(
            3,
            None,
            json!([{
                "index": 0,
                "instructions": [
                    { "programIdIndex": 4, "accounts": [], "data": "0OIl", "stackHeight": 2 },
                    compiled_ix(&[9, 9]),
                ],
            }]),
        );
        let raw = to_raw_tx("sig-5", tx).expect("meta present");
        assert_eq!(raw.inner_ix_blobs, vec![vec![9, 9]]);
    }
}
