//! Recent-fills subscription (read-only, display-only).
//!
//! A background thread subscribes to the program's `emit_cpi!` `FillEvent`s —
//! the same self-CPI inner-instruction stream the maker bot reads
//! (`bots/maker-bot/src/fills.rs`) — and forwards each decoded fill to the
//! event loop, which keeps a per-market ring for the recent-fills pane. Unlike
//! the maker's, it does **not** filter by `quote_authority`: it keeps every
//! fill (each `FillEvent` carries its `market`), and the pane filters to the
//! selected market. It never signs — it only watches.
//!
//! `emit_cpi!` records events as *inner* instructions, so a plain
//! `logsSubscribe` only learns a transaction touched the program; the thread
//! then `getTransaction`s and walks the inner instructions, verifying each
//! event's emitting program is ours before trusting the bytes (the tag and
//! discriminator are public). The `[tag][discriminator][body]` decode is the
//! shared SDK codec ([`dropset_sdk::events`]).

use crate::job::JobEvent;
use anyhow::{anyhow, Context, Result};
use dropset_sdk::events::{decode_event_payload, strip_event_tag, DropsetEvent};
use dropset_sdk::types::FillEvent;
use dropset_sdk::DROPSET_ID;
use solana_client::pubsub_client::PubsubClient;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{
    RpcTransactionConfig, RpcTransactionLogsConfig, RpcTransactionLogsFilter,
};
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction_status_client_types::{
    option_serializer::OptionSerializer, UiInnerInstructions, UiInstruction, UiLoadedAddresses,
    UiTransactionEncoding,
};
use std::str::FromStr;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Wait this long before re-subscribing after the websocket drops or a
/// subscribe attempt fails (e.g. the validator isn't up yet, or was wiped).
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Spawn the fill-subscription thread. It owns its own [`RpcClient`] and the
/// blocking pubsub subscription, reconnecting on drop — so it survives a wipe
/// (the respawned validator serves the same URL) without the event loop doing
/// anything. Forwards each decoded fill as a [`JobEvent::Fill`]; when the
/// event loop's receiver is gone (the TUI is quitting) the thread returns.
pub fn spawn(rpc_url: String, tx: Sender<JobEvent>) {
    std::thread::Builder::new()
        .name("dropset-tui-fills".into())
        .spawn(move || {
            let ws_url = ws_url_from_rpc(&rpc_url);
            let rpc = crate::chain::rpc(&rpc_url);
            run(&ws_url, &rpc, &tx);
        })
        .ok();
}

/// Derive the PubSub websocket endpoint from an RPC URL, matching the Agave
/// convention: swap the scheme (`http`→`ws`, `https`→`wss`) and use the RPC
/// port + 1 (`8899` → `8900`). Returns the input unchanged for an unrecognized
/// scheme (assume it is already a ws endpoint) or a non-numeric port. Mirrors
/// the maker bot's `config::ws_url_from_rpc`.
fn ws_url_from_rpc(rpc_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = rpc_url.strip_prefix("https://") {
        ("wss://", rest)
    } else if let Some(rest) = rpc_url.strip_prefix("http://") {
        ("ws://", rest)
    } else {
        return rpc_url.to_string();
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let ws_authority = match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => format!("{host}:{}", port.saturating_add(1)),
            Err(_) => authority.to_string(),
        },
        None => authority.to_string(),
    };
    format!("{scheme}{ws_authority}")
}

/// Subscribe, forward fills, and reconnect on websocket drop until the event
/// loop's receiver is gone (TUI quitting).
fn run(ws_url: &str, rpc: &RpcClient, tx: &Sender<JobEvent>) {
    loop {
        match subscribe_and_forward(ws_url, rpc, tx) {
            Ok(true) => return, // receiver dropped — TUI is quitting
            Ok(false) => {}     // websocket closed — reconnect
            Err(_) => {}        // subscribe failed (validator not up / wiped)
        }
        std::thread::sleep(RECONNECT_DELAY);
    }
}

/// Open one logs subscription and forward every decoded fill until it closes.
/// Returns `Ok(true)` if the event loop's receiver was dropped (stop), or
/// `Ok(false)` if the websocket closed (reconnect).
fn subscribe_and_forward(ws_url: &str, rpc: &RpcClient, tx: &Sender<JobEvent>) -> Result<bool> {
    let (_subscription, notifications) = PubsubClient::logs_subscribe(
        ws_url,
        RpcTransactionLogsFilter::Mentions(vec![DROPSET_ID.to_string()]),
        RpcTransactionLogsConfig {
            commitment: Some(CommitmentConfig::confirmed()),
        },
    )
    .map_err(|e| anyhow!("logs_subscribe {ws_url}: {e}"))?;

    for notification in notifications {
        let logs = notification.value;
        // A failed transaction commits no fills — its events are rolled back.
        if logs.err.is_some() {
            continue;
        }
        let Ok(signature) = Signature::from_str(&logs.signature) else {
            continue;
        };
        // A decode failure is non-fatal: skip the transaction and keep watching.
        let (fills, cu) = decode_fills(rpc, &signature).unwrap_or_default();
        if fills.is_empty() {
            continue;
        }
        let sig = signature.to_string();
        // A transaction that emitted fills is a swap — surface its measured CU
        // in the CU pane under "swap" (keyed by label, so a taker's swaps update
        // the same row the probe swap does), then forward each fill leg.
        if let Some(units) = cu {
            if tx
                .send(JobEvent::Cu {
                    label: "swap".to_string(),
                    units,
                    signature: sig.clone(),
                })
                .is_err()
            {
                return Ok(true);
            }
        }
        for event in fills {
            if tx
                .send(JobEvent::Fill {
                    signature: sig.clone(),
                    event,
                })
                .is_err()
            {
                return Ok(true);
            }
        }
    }
    // The notification channel closed: the websocket dropped.
    Ok(false)
}

/// Fetch the transaction and decode every `FillEvent` inner instruction our
/// program emitted, verifying each event's emitting program is `DROPSET_ID`
/// (resolved against the transaction's full account-key list) before trusting
/// its bytes — the tag and discriminator are both public. Also returns the
/// transaction's actual compute-units-consumed, so a swap's cost lands in the
/// CU pane whoever sent it (the probe swap or a taker bot).
fn decode_fills(rpc: &RpcClient, signature: &Signature) -> Result<(Vec<FillEvent>, Option<u64>)> {
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

    let tx = confirmed.transaction;
    let Some(meta) = tx.meta else {
        return Ok((Vec::new(), None));
    };
    let cu = match meta.compute_units_consumed {
        OptionSerializer::Some(units) => Some(units),
        _ => None,
    };
    let OptionSerializer::Some(inner_sets) = meta.inner_instructions else {
        return Ok((Vec::new(), cu));
    };
    // Resolve the full account-key list so each event's emitting program can be
    // checked before its bytes are trusted; bail (attribute nothing) if it
    // can't be built rather than trust an unverified emitter.
    let Some(decoded) = tx.transaction.decode() else {
        return Ok((Vec::new(), cu));
    };
    let Some(account_keys) = full_account_keys(
        decoded.message.static_account_keys(),
        &meta.loaded_addresses,
    ) else {
        return Ok((Vec::new(), cu));
    };
    Ok((collect_fills(&inner_sets, &account_keys), cu))
}

/// Walk the inner-instruction sets and collect every `FillEvent` our program
/// emitted. Split out so the emitting-program check is unit-testable without an
/// [`RpcClient`]: `account_keys` is the already-resolved full key list that
/// each instruction's `program_id_index` addresses into.
fn collect_fills(inner_sets: &[UiInnerInstructions], account_keys: &[Pubkey]) -> Vec<FillEvent> {
    let mut fills = Vec::new();
    for set in inner_sets {
        for instruction in &set.instructions {
            // `emit_cpi!` records events as compiled inner instructions.
            let UiInstruction::Compiled(compiled) = instruction else {
                continue;
            };
            // Only events emitted by our own program count — the tag and
            // discriminator are public, so the emitting program id is what
            // `emit_cpi!`'s self-CPI authenticates.
            if account_keys.get(compiled.program_id_index as usize) != Some(&DROPSET_ID) {
                continue;
            }
            // Inner-instruction data is base58 even under base64 tx encoding.
            let Ok(data) = bs58::decode(&compiled.data).into_vec() else {
                continue;
            };
            let Some(payload) = strip_event_tag(&data) else {
                continue;
            };
            if let Some(DropsetEvent::Fill(event)) = decode_event_payload(payload) {
                fills.push(event);
            }
        }
    }
    fills
}

/// Assemble the transaction's full account-key list in the order an
/// instruction's `program_id_index` addresses: the message's static keys first,
/// then the address-lookup-table loaded addresses (writable, then readonly).
/// Returns `None` if a loaded address won't parse — the caller then can't safely
/// attribute and skips the transaction rather than trust an unverified emitter.
fn full_account_keys(
    static_keys: &[Pubkey],
    loaded: &OptionSerializer<UiLoadedAddresses>,
) -> Option<Vec<Pubkey>> {
    let mut keys = static_keys.to_vec();
    if let OptionSerializer::Some(loaded) = loaded {
        for encoded in loaded.writable.iter().chain(loaded.readonly.iter()) {
            keys.push(Pubkey::from_str(encoded).ok()?);
        }
    }
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_transaction_status_client_types::UiCompiledInstruction;

    /// A `FillEvent` with recognizable field values.
    fn sample_event(market: Pubkey) -> FillEvent {
        FillEvent {
            market,
            taker: Pubkey::new_from_array([2; 32]),
            leader: Pubkey::new_from_array([3; 32]),
            quote_authority: Pubkey::new_from_array([4; 32]),
            side: 0,
            pad: [0; 7],
            sector_idx: 0,
            level_idx: 0,
            fill_base: 1_000_000,
            fill_quote: 1_140_000,
            fill_price: 0x1234_5678,
            pad2: [0; 4],
            base_atoms_after: 0,
            quote_atoms_after: 0,
            nonce_after: 1,
            taker_fee_atoms: 0,
        }
    }

    /// Wrap an event body in the `tag ++ discriminator ++ body` envelope an
    /// `emit_cpi!` inner instruction carries, and base58-encode it as the
    /// inner-instruction `data` field does.
    fn compiled_event_ix(program_id_index: u8, event: &FillEvent) -> UiInstruction {
        const FILL_DISCRIMINATOR: [u8; 8] = [13, 89, 41, 228, 105, 178, 45, 112];
        let mut data = dropset_sdk::events::EVENT_IX_TAG_LE.to_vec();
        data.extend_from_slice(&FILL_DISCRIMINATOR);
        borsh::to_writer(&mut data, event).unwrap();
        UiInstruction::Compiled(UiCompiledInstruction {
            program_id_index,
            accounts: Vec::new(),
            data: bs58::encode(data).into_string(),
            stack_height: None,
        })
    }

    /// A `FillEvent` is collected only when its inner instruction's
    /// `program_id_index` resolves to `DROPSET_ID` — a byte-identical event
    /// emitted by any other program (a spoof) is dropped.
    #[test]
    fn collects_only_events_our_program_emitted() {
        let market = Pubkey::new_from_array([1; 32]);
        let event = sample_event(market);
        // index 0 is our program, index 1 a foreign program in the same tx.
        let account_keys = vec![DROPSET_ID, Pubkey::new_from_array([9; 32])];
        let inner_sets = vec![UiInnerInstructions {
            index: 0,
            instructions: vec![
                compiled_event_ix(1, &event), // forged: emitted by the foreigner
                compiled_event_ix(0, &event), // genuine: our self-CPI
            ],
        }];
        let fills = collect_fills(&inner_sets, &account_keys);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].market, market);
    }

    /// An out-of-range `program_id_index` resolves to no account, so the event
    /// is dropped rather than indexing out of bounds.
    #[test]
    fn drops_events_with_an_out_of_range_program_index() {
        let event = sample_event(Pubkey::new_from_array([1; 32]));
        let account_keys = vec![DROPSET_ID];
        let inner_sets = vec![UiInnerInstructions {
            index: 0,
            instructions: vec![compiled_event_ix(7, &event)],
        }];
        assert!(collect_fills(&inner_sets, &account_keys).is_empty());
    }

    #[test]
    fn ws_url_follows_the_agave_convention() {
        assert_eq!(
            ws_url_from_rpc("http://127.0.0.1:8899"),
            "ws://127.0.0.1:8900"
        );
        assert_eq!(ws_url_from_rpc("ws://host:9000"), "ws://host:9000");
    }
}
