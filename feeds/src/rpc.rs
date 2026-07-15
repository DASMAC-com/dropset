//! The RPC-poll transport (`rpc` feature): poll `getSignaturesForAddress` +
//! `getTransaction` at `finalized`, generalized over program id. Extracted
//! from `indexer/src/ingest.rs` (docs/data-feeds.md §2, §4).

// cspell:word nonblocking

use crate::cursor::Cursor;
use crate::record::Batch;
use crate::source::Source;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, UiInstruction, UiTransactionEncoding,
};
use std::str::FromStr;

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
/// **Backfill windowing.** A poll takes the newest `batch_limit` signatures
/// since the cursor and advances to the newest, so a backlog larger than one
/// batch skips the middle — the same limitation the indexer has today
/// (`indexer.md` §9). A paged-backfill helper that every poll source inherits
/// is a tracked follow-up (docs/data-feeds.md §7); this source is at parity
/// with the current indexer until then.
pub struct RpcPollSource {
    name: String,
    client: RpcClient,
    program_id: Pubkey,
    batch_limit: usize,
    /// The newest signature already returned; the `until` bound next poll.
    last_signature: Option<Signature>,
}

impl RpcPollSource {
    /// A source polling `program_id` at `rpc_url`, up to `batch_limit`
    /// signatures per poll. Starts from the present; use [`Self::resume_from`]
    /// to continue from a saved cursor.
    pub fn new(rpc_url: String, program_id: Pubkey, batch_limit: usize) -> Self {
        Self {
            name: format!("rpc:{program_id}"),
            client: RpcClient::new_with_commitment(rpc_url, CommitmentConfig::finalized()),
            program_id,
            batch_limit,
            last_signature: None,
        }
    }

    /// Resume from a cursor loaded from the store at startup.
    pub fn resume_from(mut self, cursor: &Cursor) -> Result<Self> {
        let c: RpcCursor = cursor.get()?;
        self.last_signature = Some(Signature::from_str(&c.last_signature)?);
        Ok(self)
    }
}

#[async_trait]
impl Source for RpcPollSource {
    type Record = RawTx;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<RawTx>> {
        let config = GetConfirmedSignaturesForAddress2Config {
            before: None,
            until: self.last_signature,
            limit: Some(self.batch_limit),
            commitment: Some(CommitmentConfig::finalized()),
        };
        let mut sigs = self
            .client
            .get_signatures_for_address_with_config(&self.program_id, config)
            .await?;
        if sigs.is_empty() {
            return Ok(Batch::new(vec![]));
        }
        // A full page means a backlog may remain; the runner should keep polling
        // rather than sleep. Until the paged-backfill helper lands this only
        // buys one extra empty poll — the cursor still advances to the newest
        // below, so the skipped middle isn't re-fetched (see the struct doc).
        let caught_up = sigs.len() < self.batch_limit;
        // RPC returns newest-first: remember the newest, process oldest-first.
        let newest = Signature::from_str(&sigs[0].signature)?;
        sigs.reverse();

        let tx_config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Base64),
            commitment: Some(CommitmentConfig::finalized()),
            max_supported_transaction_version: Some(0),
        };
        let mut out = Vec::new();
        for s in &sigs {
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
        self.last_signature = Some(newest);
        let cursor = Cursor::new(&RpcCursor {
            last_signature: newest.to_string(),
        })?;
        Ok(Batch::new(out)
            .with_cursor(cursor)
            .with_caught_up(caught_up))
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
