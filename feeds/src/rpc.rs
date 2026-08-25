//! The RPC-poll transport (`rpc` feature): poll `getSignaturesForAddress` +
//! `getTransaction` at `finalized`, generalized over program id. Extracted
//! from `indexer/src/ingest.rs` (docs/data-feeds.md §2, §4).

// cspell:word nonblocking

use crate::backfill::{Backfill, BackfillStep};
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

/// `getSignaturesForAddress`'s documented maximum `limit`. Requesting more is
/// not a bigger page — a node either rejects the request or silently clamps
/// it, and a silent clamp is the dangerous one: every page would come back
/// short, so the walk would read "reached the bound" after a single page and
/// advance the cursor over the entire remaining backlog. Clamping here makes
/// the saturation test mean what it says.
const MAX_SIGNATURES_PER_PAGE: usize = 1000;

/// One page of the backward walk: signature summaries, newest-first, as the
/// RPC returns them.
type SignaturePage = Vec<RpcConfirmedTransactionStatusWithSignature>;

/// The address of one enumerated page — **both** ends of its window.
///
/// The `before` marker alone is not an address. A window bounded only from
/// below by the resume cursor is open at the tip, so between enumeration and
/// emission it can *grow*: the re-request would return the newest
/// `batch_limit` of a larger window, dropping the page's own oldest records
/// while the cursor advanced to the ledger's newest. Recording the page's
/// newest signature closes the window, and it is that recorded value — never
/// whatever is newest at emission time — the cursor advances to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PageKey {
    /// The `before` bound this page was fetched with; `None` is the tip.
    before: Option<Signature>,
    /// The page's newest signature when the walk saw it.
    newest: Signature,
}

/// The two RPC calls this source makes, behind a seam so the whole drive
/// loop — the `before` chaining, the saturation test, the page cap, the
/// emission and cursor advance — is exercisable without a validator. The
/// production implementation is the one below, on the real client.
///
/// It is public because [`RpcPollSource`] is generic over it, and because it
/// is the shape a geyser transport would implement: same `before`-chained
/// signature walk and per-signature fetch, different wire.
#[async_trait]
pub trait RpcTransport: Send + Sync {
    async fn page(
        &self,
        program_id: &Pubkey,
        config: GetConfirmedSignaturesForAddress2Config,
    ) -> Result<SignaturePage>;

    async fn transaction(
        &self,
        signature: &Signature,
        config: RpcTransactionConfig,
    ) -> Result<EncodedConfirmedTransactionWithStatusMeta>;
}

#[async_trait]
impl RpcTransport for RpcClient {
    async fn page(
        &self,
        program_id: &Pubkey,
        config: GetConfirmedSignaturesForAddress2Config,
    ) -> Result<SignaturePage> {
        Ok(self
            .get_signatures_for_address_with_config(program_id, config)
            .await?)
    }

    async fn transaction(
        &self,
        signature: &Signature,
        config: RpcTransactionConfig,
    ) -> Result<EncodedConfirmedTransactionWithStatusMeta> {
        Ok(self.get_transaction_with_config(signature, config).await?)
    }
}

/// One segment of the backward walk: what it found, where to resume, and
/// whether it got to the bound.
struct WalkSegment {
    /// The pages discovered, in walk order (newest page first).
    keys: Vec<PageKey>,
    /// The `before` to continue from, if the walk stopped at `max_pages`.
    resume: Option<Signature>,
    /// Whether the walk reached the resume cursor (or the start of history).
    reached_bound: bool,
}

/// Enumerate backwards from `start_before` toward `until`, at most
/// `max_pages` pages.
///
/// Signature summaries only — no transaction is fetched — and only each
/// page's two bounds are kept, so enumerating a deep backlog costs two
/// signatures per page rather than one per record. That is what lets the walk
/// run all the way to the bound, which [`Backfill`] requires before anything
/// may be emitted.
async fn enumerate_pages<P: RpcTransport>(
    client: &P,
    program_id: &Pubkey,
    until: Option<Signature>,
    start_before: Option<Signature>,
    batch_limit: usize,
    max_pages: usize,
) -> Result<WalkSegment> {
    let mut keys = Vec::new();
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
                keys,
                resume: before,
                reached_bound: true,
            });
        }
        keys.push(PageKey {
            before,
            newest: Signature::from_str(&page[0].signature)?,
        });
        // A short page means the window between `before` and the cursor is
        // exhausted: the walk has reached the bound (or the start of the
        // program's history).
        let saturated = page.len() >= batch_limit;
        before = Some(Signature::from_str(&page[page.len() - 1].signature)?);
        if !saturated {
            return Ok(WalkSegment {
                keys,
                resume: before,
                reached_bound: true,
            });
        }
    }
    Ok(WalkSegment {
        keys,
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
/// each page's two bounds, until it reaches the resume cursor; then it
/// *hydrates* one page per [`Source::next`], oldest page first, advancing
/// the cursor to the newest signature that page held **when it was
/// enumerated**. Because emission runs oldest → newest and the cursor lands
/// on a recorded bound rather than on whatever is newest at emission time,
/// nothing is ever left behind the cursor.
///
/// Enumeration is bounded per poll, not in total: a walk that hits
/// `DEFAULT_MAX_PAGES_PER_POLL` resumes where it stopped on the next poll
/// (reporting a backlog, so the runner loops straight back) rather than
/// emitting from an unknown position.
pub struct RpcPollSource<T = RpcClient> {
    name: String,
    client: T,
    program_id: Pubkey,
    batch_limit: usize,
    max_pages_per_poll: usize,
    /// The newest signature already emitted; the `until` bound on every
    /// request until it moves.
    last_signature: Option<Signature>,
    /// Where an unfinished backward walk resumes from.
    walk_before: Option<Signature>,
    /// Pages enumerated but not yet emitted, drained oldest-first.
    pager: Backfill<PageKey>,
}

impl RpcPollSource<RpcClient> {
    /// A source polling `program_id` at `rpc_url`, up to `batch_limit`
    /// signatures per page. Starts from the present; use [`Self::resume_from`]
    /// to continue from a saved cursor.
    pub fn new(rpc_url: String, program_id: Pubkey, batch_limit: usize) -> Self {
        let client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::finalized());
        Self::with_transport(client, program_id, batch_limit)
    }
}

impl<T: RpcTransport> RpcPollSource<T> {
    /// A source over an arbitrary transport. [`Self::new`] is this with the
    /// real client; the seam exists so the drive loop can be tested without a
    /// validator.
    fn with_transport(client: T, program_id: Pubkey, batch_limit: usize) -> Self {
        Self {
            name: format!("rpc:{program_id}"),
            client,
            program_id,
            // A zero page would make every page look saturated and the walk
            // never reach the bound; asking for more than the RPC's maximum
            // risks a silent clamp, which has the same effect in reverse —
            // every page short, so the walk stops after one.
            batch_limit: batch_limit.clamp(1, MAX_SIGNATURES_PER_PAGE),
            max_pages_per_poll: DEFAULT_MAX_PAGES_PER_POLL,
            last_signature: None,
            walk_before: None,
            pager: Backfill::new(),
        }
    }

    /// Cap how many signature pages one poll enumerates before yielding.
    /// Raising it reaches the bound in fewer polls at the cost of a longer
    /// single `next()`; the default is `DEFAULT_MAX_PAGES_PER_POLL`.
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

    /// Re-request one enumerated page and trim it back to what the walk saw.
    ///
    /// The current resume cursor bounds the window from below, but the tip
    /// page's window is open above, so records that landed since enumeration
    /// appear at its head. Those belong to a later walk — the cursor is about
    /// to stop at this page's recorded `newest`, which leaves them above it —
    /// so they are dropped here rather than emitted.
    ///
    /// `Ok(None)` means the page could not be reproduced: its recorded newest
    /// is no longer inside the window (more than `batch_limit` arrivals since
    /// enumeration), or the window came back empty (a pooled endpoint with
    /// shallower history). Neither is safe to emit from, and neither may
    /// advance the cursor — the caller re-enumerates instead.
    async fn fetch_page(&self, key: &PageKey) -> Result<Option<SignaturePage>> {
        let config = GetConfirmedSignaturesForAddress2Config {
            before: key.before,
            until: self.last_signature,
            limit: Some(self.batch_limit),
            commitment: Some(CommitmentConfig::finalized()),
        };
        let page = self.client.page(&self.program_id, config).await?;
        let recorded = key.newest.to_string();
        match page.iter().position(|s| s.signature == recorded) {
            Some(head) => Ok(Some(page[head..].to_vec())),
            None => Ok(None),
        }
    }

    /// Fetch each transaction in `page` and flatten it, oldest-first.
    /// Transactions that failed on-chain carry no events worth storing and
    /// are skipped.
    async fn hydrate(&self, page: &SignaturePage) -> Result<Vec<RawTx>> {
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
            let tx = self.client.transaction(&signature, tx_config).await?;
            if let Some(raw) = to_raw_tx(&s.signature, tx) {
                out.push(raw);
            }
        }
        Ok(out)
    }

    /// Abandon the current walk and enumerate again from the present. The
    /// cursor is untouched, so nothing already emitted is re-emitted and
    /// nothing pending is skipped.
    fn restart_walk(&mut self) {
        self.pager.restart();
        self.walk_before = None;
    }
}

#[async_trait]
impl<T: RpcTransport> Source for RpcPollSource<T> {
    type Record = RawTx;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<RawTx>> {
        loop {
            match self.pager.step() {
                BackfillStep::Enumerate => {
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
                    self.pager.extend(segment.keys, segment.reached_bound);
                    if !segment.reached_bound {
                        // Still enumerating, so nothing may be emitted yet.
                        // Report a backlog and let the runner loop straight
                        // back rather than sleeping mid-walk.
                        return Ok(Batch::new(vec![]).with_caught_up(false));
                    }
                }
                BackfillStep::Emit(key) => {
                    let Some(page) = self.fetch_page(&key).await? else {
                        // The page could not be reproduced from its window.
                        // Emitting a different page under its cursor would
                        // advance over the difference, so drop the walk and
                        // enumerate again from the present instead. The
                        // cursor has not moved, so nothing is lost.
                        self.restart_walk();
                        return Ok(Batch::new(vec![]).with_caught_up(false));
                    };
                    let out = self.hydrate(&page).await?;

                    // Everything above is fallible, and the runner keeps this
                    // source alive across an error — so the page stays queued
                    // until here. Committing after the batch is built is what
                    // makes a failed emission a retry rather than a skip.
                    self.pager.commit();
                    // The cursor advances to the newest signature the page
                    // held *when it was enumerated*, never to whatever is
                    // newest now: anything that landed since is above it and
                    // belongs to the next walk.
                    self.last_signature = Some(key.newest);
                    let cursor = Cursor::new(&RpcCursor {
                        last_signature: key.newest.to_string(),
                    })?;
                    return Ok(Batch::new(out)
                        .with_cursor(cursor)
                        .with_caught_up(self.pager.caught_up()));
                }
                BackfillStep::Done => {
                    // Backlog drained; the next poll walks from the present.
                    self.restart_walk();
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
    use std::sync::Mutex;

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

    /// A stand-in cluster: signatures newest-first, paged exactly as
    /// `getSignaturesForAddress` does — `before` excludes everything at or
    /// newer than it, `until` excludes everything at or older than it, and at
    /// most `limit` come back, **taken from the newest end**. That last
    /// detail is the one the backfill design turns on, so the fake models it
    /// explicitly.
    ///
    /// It is deliberately mutable and fallible: the two failures this
    /// transport must survive — a page that errors mid-drain, and records
    /// landing while a backlog drains — are invisible to a fixed, infallible
    /// fake.
    #[derive(Default)]
    struct FakeRpc {
        state: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        /// Every signature the program has, newest-first.
        signatures: Vec<String>,
        /// How many `page` calls have been made.
        page_calls: usize,
        /// The `page` call index that returns an error instead of a page.
        fail_page_call: Option<usize>,
        /// After this `page` call index, append `grow_count` fresh
        /// signatures at the tip — a burst arriving mid-drain.
        grow_after_page_call: Option<usize>,
        grow_count: usize,
        /// Distinguishes minted signatures from the seeded ones.
        minted: u64,
    }

    /// A distinct, valid signature for index `i`.
    fn sig_at(i: u64) -> String {
        let mut bytes = [0u8; 64];
        bytes[..8].copy_from_slice(&i.to_be_bytes());
        Signature::from(bytes).to_string()
    }

    impl FakeRpc {
        /// `n` signatures, newest-first — index 0 is the newest.
        fn with_signatures(n: usize) -> Self {
            let fake = Self::default();
            fake.state.lock().unwrap().signatures = (0..n as u64).map(sig_at).collect();
            fake
        }

        fn fail_page_call(self, call: usize) -> Self {
            self.state.lock().unwrap().fail_page_call = Some(call);
            self
        }

        fn grow_after_page_call(self, call: usize, count: usize) -> Self {
            let mut state = self.state.lock().unwrap();
            state.grow_after_page_call = Some(call);
            state.grow_count = count;
            drop(state);
            self
        }

        /// The signatures present at construction, oldest-first — what a
        /// correct drain must emit, in order.
        fn seeded_oldest_first(&self, n: usize) -> Vec<String> {
            (0..n as u64).map(sig_at).rev().collect()
        }
    }

    #[async_trait]
    impl RpcTransport for FakeRpc {
        async fn page(
            &self,
            _program_id: &Pubkey,
            config: GetConfirmedSignaturesForAddress2Config,
        ) -> Result<SignaturePage> {
            let mut state = self.state.lock().unwrap();
            let call = state.page_calls;
            state.page_calls += 1;
            if state.fail_page_call == Some(call) {
                anyhow::bail!("scripted page failure on call {call}");
            }

            let position = |sig: Signature, haystack: &[String]| {
                let s = sig.to_string();
                haystack.iter().position(|have| *have == s)
            };
            let start = config
                .before
                .and_then(|b| position(b, &state.signatures))
                .map_or(0, |i| i + 1);
            let end = config
                .until
                .and_then(|u| position(u, &state.signatures))
                .unwrap_or(state.signatures.len());
            let limit = config.limit.unwrap_or(state.signatures.len());
            let page: SignaturePage = state.signatures[start.min(end)..end]
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
                .collect();

            if state.grow_after_page_call == Some(call) {
                for _ in 0..state.grow_count {
                    state.minted += 1;
                    let fresh = sig_at(1_000 + state.minted);
                    state.signatures.insert(0, fresh);
                }
            }
            Ok(page)
        }

        async fn transaction(
            &self,
            _signature: &Signature,
            _config: RpcTransactionConfig,
        ) -> Result<EncodedConfirmedTransactionWithStatusMeta> {
            Ok(encoded_tx(1, None, json!([])))
        }
    }

    fn source_over(fake: FakeRpc, batch_limit: usize) -> RpcPollSource<FakeRpc> {
        RpcPollSource::with_transport(fake, Pubkey::new_unique(), batch_limit)
    }

    /// Drive the **real** [`Source::next`] to exhaustion, collecting the
    /// signature of every record it emits, in emission order.
    ///
    /// It mirrors the runner's error handling deliberately: a `next()` that
    /// errors is logged and retried against the *same* source, which is what
    /// makes a dropped page observable here rather than only in production.
    async fn drain_source(source: &mut RpcPollSource<FakeRpc>, max_polls: usize) -> Vec<String> {
        let mut emitted = Vec::new();
        for _ in 0..max_polls {
            let batch = match source.next().await {
                Ok(batch) => batch,
                // What `run_until` does: warn, back off, poll again.
                Err(_) => continue,
            };
            emitted.extend(batch.records.iter().map(|r| r.signature.clone()));
            if batch.caught_up && batch.is_empty() {
                return emitted;
            }
        }
        emitted
    }

    /// A backlog that fits in one page needs one call and reaches the bound.
    #[tokio::test]
    async fn a_short_page_ends_the_walk_at_the_bound() {
        let fake = FakeRpc::with_signatures(3);
        let segment = enumerate_pages(&fake, &Pubkey::new_unique(), None, None, 10, 16)
            .await
            .unwrap();

        assert!(segment.reached_bound);
        assert_eq!(segment.keys.len(), 1);
        assert_eq!(segment.keys[0].before, None);
        // The key pins the page's newest signature, not just its lower bound.
        assert_eq!(segment.keys[0].newest.to_string(), sig_at(0));
    }

    /// The walk chains `before` from each page's oldest signature, so the keys
    /// address consecutive, non-overlapping windows — each pinned at both ends.
    #[tokio::test]
    async fn the_walk_chains_page_keys_across_pages() {
        let fake = FakeRpc::with_signatures(7);
        let segment = enumerate_pages(&fake, &Pubkey::new_unique(), None, None, 3, 16)
            .await
            .unwrap();

        assert!(segment.reached_bound);
        // 3 + 3 + 1: the short last page is what ends the walk.
        let sig = |i: u64| Signature::from_str(&sig_at(i)).unwrap();
        let expected = vec![
            PageKey {
                before: None,
                newest: sig(0),
            },
            PageKey {
                before: Some(sig(2)),
                newest: sig(3),
            },
            PageKey {
                before: Some(sig(5)),
                newest: sig(6),
            },
        ];
        assert_eq!(segment.keys, expected);
    }

    /// A walk stops at the resume cursor: signatures at or older than it were
    /// already emitted and must not be enumerated again.
    #[tokio::test]
    async fn the_walk_stops_at_the_resume_cursor() {
        let fake = FakeRpc::with_signatures(6);
        // Resume just below the three newest.
        let until = Signature::from_str(&sig_at(3)).unwrap();
        let segment = enumerate_pages(&fake, &Pubkey::new_unique(), Some(until), None, 2, 16)
            .await
            .unwrap();

        assert!(segment.reached_bound);
        let bounds: Vec<_> = segment.keys.iter().map(|k| k.before).collect();
        assert_eq!(
            bounds,
            vec![None, Some(Signature::from_str(&sig_at(1)).unwrap())]
        );
    }

    /// Hitting the per-poll cap reports an unreached bound and where to
    /// resume, so the next poll continues the same walk instead of
    /// restarting or emitting from an unknown position.
    #[tokio::test]
    async fn hitting_the_per_poll_cap_reports_where_to_resume() {
        let fake = FakeRpc::with_signatures(100);
        let segment = enumerate_pages(&fake, &Pubkey::new_unique(), None, None, 5, 3)
            .await
            .unwrap();

        assert!(!segment.reached_bound);
        assert_eq!(segment.keys.len(), 3);
        // Three pages of five consumed the fifteen newest signatures.
        assert_eq!(
            segment.resume,
            Some(Signature::from_str(&sig_at(14)).unwrap())
        );
    }

    /// The regression this whole change exists for: a backlog deeper than one
    /// page is emitted in full, oldest-first, with nothing skipped. The old
    /// source took the newest page and jumped the cursor to its newest
    /// signature, losing every record below it.
    #[tokio::test]
    async fn a_deep_backlog_drains_completely_and_in_order() {
        let mut source = source_over(FakeRpc::with_signatures(9), 2);
        let emitted = drain_source(&mut source, 64).await;
        assert_eq!(emitted, source.client.seeded_oldest_first(9));
    }

    /// The same guarantee when the per-poll cap splits the walk across
    /// several polls: enumeration resumes where it stopped, and emission
    /// still begins at the true oldest record.
    #[tokio::test]
    async fn a_walk_split_across_polls_still_drains_in_order() {
        // Page size 2 with a 2-page cap: the walk needs three segments.
        let mut source = source_over(FakeRpc::with_signatures(9), 2).with_max_pages_per_poll(2);
        let emitted = drain_source(&mut source, 64).await;
        assert_eq!(emitted, source.client.seeded_oldest_first(9));
    }

    /// Records landing while a backlog drains must not displace it.
    ///
    /// The tip page's window is the one bounded only from below, and it is
    /// emitted last — so on a deep backlog it is re-requested minutes after it
    /// was enumerated. If the cursor were taken from whatever is newest at
    /// that moment, the arrivals would push the page's own oldest records
    /// below it and they would never be enumerated again. The page key pins
    /// the newest signature the walk actually saw, so the cursor stops there
    /// and the arrivals stay above it for the following walk.
    #[tokio::test]
    async fn records_landing_during_a_drain_do_not_displace_the_backlog() {
        // 9 signatures at 2 per page: 5 enumeration calls (the last short),
        // then emission re-requests each page. Grow the tip right after
        // enumeration finishes, while the oldest pages are still draining.
        let fake = FakeRpc::with_signatures(9).grow_after_page_call(5, 3);
        let mut source = source_over(fake, 2);

        let emitted = drain_source(&mut source, 64).await;

        // Every seeded record is emitted, in order, before any new arrival.
        let seeded = source.client.seeded_oldest_first(9);
        assert!(
            emitted.len() >= seeded.len(),
            "expected at least the seeded backlog, got {} records",
            emitted.len()
        );
        assert_eq!(&emitted[..seeded.len()], &seeded[..]);
    }

    /// An error partway through emitting a page must retry that page, not
    /// skip it.
    ///
    /// The runner keeps the source alive across a source error, so a page
    /// discarded on the way out is never revisited — and the cursor, which
    /// only moves on success, would later advance straight over it. The
    /// pager therefore holds the page until the batch is built.
    #[tokio::test]
    async fn a_failed_emission_retries_the_same_page() {
        // Fail the first emission fetch (call 5 — calls 0-4 enumerate).
        let fake = FakeRpc::with_signatures(9).fail_page_call(5);
        let mut source = source_over(fake, 2);

        let emitted = drain_source(&mut source, 64).await;

        assert_eq!(emitted, source.client.seeded_oldest_first(9));
    }

    /// A drained backlog leaves the source able to find the next one.
    ///
    /// `Done` resets the walk's resume marker as well as the pager. Without
    /// that reset the next walk would run `before` = an old signature against
    /// `until` = a newer one — a window empty by construction — so the source
    /// would report itself current forever and silently stop indexing.
    #[tokio::test]
    async fn a_second_walk_picks_up_records_that_arrive_after_a_drain() {
        let mut source = source_over(FakeRpc::with_signatures(4), 2);
        let first = drain_source(&mut source, 64).await;
        assert_eq!(first, source.client.seeded_oldest_first(4));

        // Two more transactions land after the source went idle.
        {
            let mut state = source.client.state.lock().unwrap();
            state.signatures.insert(0, sig_at(2_001));
            state.signatures.insert(0, sig_at(2_002));
        }

        let second = drain_source(&mut source, 64).await;
        assert_eq!(second, vec![sig_at(2_001), sig_at(2_002)]);
    }

    /// A ledger with nothing newer than the cursor emits nothing and reports
    /// the source current.
    #[tokio::test]
    async fn an_empty_window_emits_nothing() {
        let mut source = source_over(FakeRpc::with_signatures(0), 5);
        assert!(drain_source(&mut source, 16).await.is_empty());
    }

    /// A page size above the RPC's documented maximum is clamped rather than
    /// sent as-is: a node that silently clamps instead of rejecting would
    /// return every page short, and the walk would read that as "reached the
    /// bound" after one page and advance over the rest of the backlog.
    #[test]
    fn an_oversized_page_request_is_clamped_to_the_rpc_maximum() {
        let source = source_over(FakeRpc::default(), 100_000);
        assert_eq!(source.batch_limit, MAX_SIGNATURES_PER_PAGE);

        let source = source_over(FakeRpc::default(), 0);
        assert_eq!(source.batch_limit, 1);
    }

    /// The cursor this source writes is the cursor it can resume from.
    ///
    /// The two halves are written apart — `next` serializes an `RpcCursor`,
    /// `resume_from` deserializes one — so nothing but this test stops a
    /// rename of that struct's field from turning every restart into a
    /// startup failure, or worse, a silent restart from the present.
    #[tokio::test]
    async fn a_cursor_this_source_wrote_resumes_this_source() {
        let mut source = source_over(FakeRpc::with_signatures(4), 2);
        drain_source(&mut source, 64).await;
        let emitted_cursor = source
            .last_signature
            .expect("the drain advanced the cursor");

        // Round-trip through the opaque form the store persists.
        let batch = Batch::new(Vec::<RawTx>::new()).with_cursor(
            Cursor::new(&RpcCursor {
                last_signature: emitted_cursor.to_string(),
            })
            .unwrap(),
        );
        let stored = batch.cursor.expect("cursor attached");

        let resumed = source_over(FakeRpc::with_signatures(4), 2)
            .resume_from(&stored)
            .expect("a cursor this source wrote must load");
        assert_eq!(resumed.last_signature, Some(emitted_cursor));

        // And a resumed source is genuinely current — it re-emits nothing.
        let mut resumed = resumed;
        assert!(drain_source(&mut resumed, 16).await.is_empty());
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
