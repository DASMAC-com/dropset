//! RPC-poll event source (docs/indexer.md §4.2): poll
//! `getSignaturesForAddress` + `getTransaction` at `finalized`, returning
//! each new transaction's inner-instruction `data` for the decoder. The
//! geyser path would implement the same `poll` shape behind the same
//! decode + store seam.

// cspell:word nonblocking

use crate::decode::RawTx;
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

pub struct RpcPollSource {
    client: RpcClient,
    program_id: Pubkey,
    batch_limit: usize,
    /// The newest signature already returned; the `until` bound next poll.
    last_signature: Option<Signature>,
}

impl RpcPollSource {
    pub fn new(rpc_url: String, program_id: Pubkey, batch_limit: usize) -> Self {
        Self {
            client: RpcClient::new_with_commitment(rpc_url, CommitmentConfig::finalized()),
            program_id,
            batch_limit,
            last_signature: None,
        }
    }

    /// One poll: new transactions oldest-first, ready to decode.
    pub async fn poll(&mut self) -> anyhow::Result<Vec<RawTx>> {
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
            return Ok(vec![]);
        }
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
        Ok(out)
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
        // The RPC path can't cheaply learn a tx's position in its block;
        // the signature already makes the PK unique, so 0 is a safe filler
        // (the geyser path supplies the true index).
        txn_index: 0,
        signature: signature.to_string(),
        block_time,
        inner_ix_blobs,
    })
}
