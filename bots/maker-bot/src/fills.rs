//! Real-time fill detection by subscribing to the program's `emit_cpi!`
//! `FillEvent`s (§3 fill detection — the production-fidelity path, full
//! fidelity, never dropped).
//!
//! The swap hot path emits one `FillEvent` per filled leg via `emit_cpi!`,
//! which anchor records as a *self-CPI*: the event bytes land in the
//! transaction's **inner instructions**, not the `Program data:` logs. So a
//! plain `logsSubscribe` only learns a transaction *touched* the program — to
//! read the events it must then `getTransaction` and walk the inner
//! instructions. Each event inner-instruction `data` is
//!
//! ```text
//! EVENT_IX_TAG_LE (8)  ++  DISCRIMINATOR (8)  ++  body
//! ```
//!
//! where the body is the borsh wire form. `FillEvent` is `#[event(bytemuck)]`
//! on-chain — a fixed `repr(C)` struct with explicit padding fields chosen so
//! it has no implicit padding, which makes its raw bytes byte-identical to the
//! borsh serialization of the SDK's generated [`FillEvent`] (`Price` is a
//! `u32`). So the body decodes straight through `BorshDeserialize`; the
//! program's own `events.rs` test pins the on-chain size, and the round-trip
//! test below pins this decoder against the same wire format.
//!
//! This runs on a dedicated thread so the `getTransaction` round-trips never
//! stall the synchronous quoting tick. It forwards each attributed fill over
//! an [`mpsc`] channel the tick drains; the per-tick inventory diff in
//! `tasks.rs` is the fallback for when the subscription is down or a fill is
//! missed.

use anchor_lang_v2::event::EVENT_IX_TAG_LE;
use anyhow::{anyhow, Context as _, Result};
use borsh::BorshDeserialize;
use dropset_sdk::types::FillEvent;
use dropset_sdk::DROPSET_ID;
use sha2::{Digest, Sha256};
use solana_client::pubsub_client::PubsubClient;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{
    RpcTransactionConfig, RpcTransactionLogsConfig, RpcTransactionLogsFilter,
};
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction_status_client_types::{
    option_serializer::OptionSerializer, UiInstruction, UiTransactionEncoding,
};
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// How long to wait before re-subscribing after the websocket drops or a
/// subscribe attempt fails (e.g. the validator isn't up yet).
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// `FillEvent`'s 8-byte event discriminator, reproduced via anchor's
/// name-based scheme: `sha256("event:<Name>")[..8]` (see anchor-lang-v2's
/// event macro, which hashes `format!("event:{name}")`). The on-chain type
/// carrying the real impl lives in the program crate, which the bot can't
/// depend on (it compiles for SBF), so the bot recomputes the same bytes.
/// Pinned end-to-end by the round-trip test below; the program's `events.rs`
/// test pins the on-chain side of the same wire format.
fn fill_event_discriminator() -> [u8; 8] {
    let hash = Sha256::digest(b"event:FillEvent");
    hash[..8].try_into().expect("sha256 digest is 32 bytes")
}

/// One attributed fill leg: a decoded [`FillEvent`] and the signature of the
/// swap that produced it (for logging / dedup).
#[derive(Clone, Debug)]
pub struct Fill {
    pub signature: Signature,
    pub event: FillEvent,
}

/// Spawn the fill-subscription thread and return the channel the tick drains.
///
/// `quote_authority` is the bot's vault key (the leader); only fills against
/// that vault are forwarded. The thread owns its own [`RpcClient`] and the
/// blocking pubsub subscription, reconnecting on drop — it never quotes.
pub fn spawn(ws_url: String, rpc_url: String, quote_authority: Pubkey) -> Receiver<Fill> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("maker-bot-fills".into())
        .spawn(move || {
            let rpc = crate::chain::rpc(&rpc_url);
            run(&ws_url, &rpc, &quote_authority, &tx);
        });
    if let Err(e) = spawned {
        eprintln!(
            "[fills] could not spawn subscription thread: {e}; using inventory-diff fallback"
        );
    }
    rx
}

/// Subscribe, forward fills, and reconnect on websocket drop until the tick's
/// receiver is gone (bot shutting down).
fn run(ws_url: &str, rpc: &RpcClient, quote_authority: &Pubkey, tx: &Sender<Fill>) {
    loop {
        match subscribe_and_forward(ws_url, rpc, quote_authority, tx) {
            Ok(true) => return, // receiver dropped — bot is shutting down
            Ok(false) => {
                eprintln!("[fills] websocket closed; reconnecting in {RECONNECT_DELAY:?}")
            }
            Err(e) => {
                eprintln!("[fills] subscription error: {e}; reconnecting in {RECONNECT_DELAY:?}")
            }
        }
        std::thread::sleep(RECONNECT_DELAY);
    }
}

/// Open one logs subscription and forward attributed fills until it closes.
/// Returns `Ok(true)` if the tick's receiver was dropped (stop), `Ok(false)`
/// if the websocket closed (reconnect).
fn subscribe_and_forward(
    ws_url: &str,
    rpc: &RpcClient,
    quote_authority: &Pubkey,
    tx: &Sender<Fill>,
) -> Result<bool> {
    let (_subscription, notifications) = PubsubClient::logs_subscribe(
        ws_url,
        RpcTransactionLogsFilter::Mentions(vec![DROPSET_ID.to_string()]),
        RpcTransactionLogsConfig {
            commitment: Some(CommitmentConfig::confirmed()),
        },
    )
    .map_err(|e| anyhow!("logs_subscribe {ws_url}: {e}"))?;
    println!("[fills] subscribed to {DROPSET_ID} logs at {ws_url}");

    for notification in notifications {
        let logs = notification.value;
        // A failed transaction commits no fills — its events are rolled back.
        if logs.err.is_some() {
            continue;
        }
        let signature = match Signature::from_str(&logs.signature) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[fills] unparseable signature {}: {e}", logs.signature);
                continue;
            }
        };
        match decode_fills(rpc, &signature, quote_authority) {
            Ok(fills) => {
                for fill in fills {
                    if tx.send(fill).is_err() {
                        return Ok(true);
                    }
                }
            }
            Err(e) => eprintln!("[fills] decode {signature}: {e}"),
        }
    }
    // The notification channel closed: the websocket dropped.
    Ok(false)
}

/// Fetch the transaction and decode every `FillEvent` inner instruction that
/// hit our vault.
fn decode_fills(
    rpc: &RpcClient,
    signature: &Signature,
    quote_authority: &Pubkey,
) -> Result<Vec<Fill>> {
    let confirmed = rpc
        .get_transaction_with_config(
            signature,
            RpcTransactionConfig {
                encoding: Some(UiTransactionEncoding::Base64),
                commitment: Some(CommitmentConfig::confirmed()),
                max_supported_transaction_version: Some(0),
            },
        )
        .context("get_transaction")?;

    let Some(meta) = confirmed.transaction.meta else {
        return Ok(Vec::new());
    };
    let OptionSerializer::Some(inner_sets) = meta.inner_instructions else {
        return Ok(Vec::new());
    };

    let mut fills = Vec::new();
    for set in &inner_sets {
        for instruction in &set.instructions {
            // `emit_cpi!` records events as compiled inner instructions.
            let UiInstruction::Compiled(compiled) = instruction else {
                continue;
            };
            // Inner-instruction data is base58 even under base64 tx encoding.
            let Ok(data) = bs58::decode(&compiled.data).into_vec() else {
                continue;
            };
            let Some(event) = decode_fill_event(&data) else {
                continue;
            };
            if event.quote_authority == *quote_authority {
                fills.push(Fill {
                    signature: *signature,
                    event,
                });
            }
        }
    }
    Ok(fills)
}

/// Decode one inner-instruction blob as a [`FillEvent`], or `None` if it is a
/// different event / not an event at all. The trailing `try_from_slice` also
/// length-checks the body (borsh rejects trailing bytes).
fn decode_fill_event(data: &[u8]) -> Option<FillEvent> {
    let discriminator = fill_event_discriminator();
    let prefix = EVENT_IX_TAG_LE.len() + discriminator.len();
    if data.len() < prefix
        || &data[..EVENT_IX_TAG_LE.len()] != EVENT_IX_TAG_LE
        || data[EVENT_IX_TAG_LE.len()..prefix] != discriminator
    {
        return None;
    }
    FillEvent::try_from_slice(&data[prefix..]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `FillEvent` with distinct, recognizable field values.
    fn sample_event(quote_authority: Pubkey) -> FillEvent {
        FillEvent {
            market: Pubkey::new_from_array([1; 32]),
            taker: Pubkey::new_from_array([2; 32]),
            leader: Pubkey::new_from_array([3; 32]),
            quote_authority,
            side: 1,
            pad: [0; 7],
            sector_idx: 4,
            level_idx: 2,
            fill_base: 1_000,
            fill_quote: 730,
            fill_price: 0x1234_5678,
            pad2: [0; 4],
            base_atoms_after: 9_000,
            quote_atoms_after: 8_000,
            nonce_after: 42,
            taker_fee_atoms: 7,
        }
    }

    /// Wrap an event body in the `tag ++ discriminator ++ body` envelope an
    /// `emit_cpi!` inner instruction carries.
    fn wrap(event: &FillEvent) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(EVENT_IX_TAG_LE);
        data.extend_from_slice(&fill_event_discriminator());
        data.extend_from_slice(&borsh::to_vec(event).unwrap());
        data
    }

    /// The borsh body is exactly the on-chain `repr(C)` size — the explicit
    /// padding fields make the two layouts byte-identical (200 bytes:
    /// 4×32-byte keys + u8 + [u8;7] + 2×u32 + 2×u64 + u32 + [u8;4] + 4×u64).
    #[test]
    fn body_is_the_fixed_event_size() {
        let body = borsh::to_vec(&sample_event(Pubkey::new_unique())).unwrap();
        assert_eq!(body.len(), 200);
    }

    #[test]
    fn decodes_a_round_tripped_fill() {
        let event = sample_event(Pubkey::new_unique());
        let decoded = decode_fill_event(&wrap(&event)).expect("should decode");
        assert_eq!(decoded, event);
    }

    #[test]
    fn rejects_a_foreign_discriminator() {
        let event = sample_event(Pubkey::new_unique());
        let mut data = wrap(&event);
        // Flip a discriminator byte: now it's some other event.
        data[EVENT_IX_TAG_LE.len()] ^= 0xff;
        assert!(decode_fill_event(&data).is_none());
    }

    #[test]
    fn rejects_a_non_event_instruction() {
        assert!(decode_fill_event(&[0u8; 4]).is_none());
        assert!(decode_fill_event(&[]).is_none());
    }
}
