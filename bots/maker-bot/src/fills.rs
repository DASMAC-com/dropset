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
//! where the body is the borsh wire form. Since that tag and the name-based
//! discriminator are both public, any program could emit a `FillEvent`-shaped
//! inner instruction, so the decoder first resolves each inner instruction's
//! `program_id_index` against the transaction's full account-key list and
//! requires it to be the dropset program before trusting the bytes.
//!
//! The `[tag][discriminator][body]` decode is the shared SDK codec
//! ([`dropset_sdk::events`]) — the same one the TUI's recent-fills
//! subscription uses. `FillEvent` is `#[event(bytemuck)]` on-chain (a fixed
//! `repr(C)` struct with explicit padding fields, so its raw bytes are
//! byte-identical to the borsh form of the SDK's generated [`FillEvent`]); the
//! SDK's own `events.rs` tests pin that wire format, and the round-trip test
//! below pins this crate's thin wrapper against it.
//!
//! This runs on a dedicated thread so the `getTransaction` round-trips never
//! stall the synchronous quoting tick. It is this bot's concrete socket for the
//! `feeds` **stream** seam (docs/data-feeds.md §4, §7): each attributed fill is
//! pushed into a [`ChannelSource`], which the framework runner fans to the
//! in-process forward (live) sink the tick drains. The per-tick inventory diff
//! in `tasks.rs` is the fallback for when the subscription is down or a fill is
//! missed.
//!
//! Being a push source, its liveness is **not** the runner's `feed_health` row
//! — that would track the last fill rather than the last healthy socket, and
//! page about a price feed on any quiet market. This thread reports its own
//! transport state through [`LivenessReporter`] instead (`push_health`): up on
//! a successful subscribe, down when the socket closes, and down-with-reason
//! when a subscribe fails or this thread ends. Silence is never an alert.

use crate::telemetry::Record;
use anyhow::{anyhow, Context as _, Result};
use dropset_feeds::{redact_to_origin, ChannelSource, LivenessReporter, MAX_ERROR_CHARS};
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
use std::time::Duration;
use tokio::sync::mpsc::Sender;

/// How long to wait before re-subscribing after the websocket drops or a
/// subscribe attempt fails (e.g. the validator isn't up yet).
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// How long the subscription waits on a notification before looping to retry
/// any liveness transition the telemetry channel refused.
///
/// **Not a staleness bound, and nothing derives one from it.** A timeout here
/// is the *normal* state of a market with no fills, which is exactly why this
/// source reports a connection state rather than a message recency. The
/// interval exists only so a dropped liveness report is retried on a cadence
/// instead of waiting for the next fill — on a quiet market that could be
/// hours, and a quiet market is precisely where a dead socket hides.
const REASSERT_INTERVAL: Duration = Duration::from_secs(30);

/// This source's `feeds` `Source::name()`, and so the key its `push_health`
/// row is upserted on.
///
/// One constant for both because the two are the same identity: the caller
/// names the liveness reporter and this module names the source, and a drift
/// between them would file the transport's state under a feed that does not
/// exist while the real one stayed silently absent — the exact failure the
/// reporting is for.
pub const FILLS_FEED: &str = "maker-fills";

/// The channel between the subscription thread and the `feeds` runner. Sized to
/// absorb a fill burst between ticks; the forward sink downstream drops to the
/// latest, and the tick keeps only the highest-`nonce_after` fill per market,
/// so a full channel loses nothing the reconcile needs.
const FILL_BUFFER: usize = 1024;

/// One attributed fill leg: a decoded [`FillEvent`] and the signature of the
/// swap that produced it (for logging / dedup).
#[derive(Clone, Debug)]
pub struct Fill {
    pub signature: Signature,
    pub event: FillEvent,
}

/// Spawn the fill-subscription thread and return the [`ChannelSource`] the
/// `feeds` runner drives into the live forward sink the tick drains.
///
/// `quote_authority` is the bot's vault key (the leader); only fills against
/// that vault are forwarded. The thread owns its own [`RpcClient`] and the
/// blocking pubsub subscription, reconnecting on drop — it never quotes. It is
/// spawned outside the tokio runtime, so its `blocking_send` into the channel
/// is a plain blocking call, never an in-runtime panic.
///
/// Returns `None` if the thread can't be spawned, so the caller leaves the
/// forward sink unset and the tick falls back to the inventory diff. The thread
/// otherwise reconnects forever, and its position bookkeeping is safe either
/// way: the per-cycle vault reconcile (`decide_position` in `tasks.rs`) tracks
/// the chain whenever the position and vault diverge.
///
/// If it dies *later* (a decode-path panic), the `feeds` stream seam reports
/// the dropped sender as an idle source, not a close, so `ctx.fills_active`
/// stays set and nothing in the record stream can tell that apart from a quiet
/// market. `liveness` is what makes that visible: it is **moved into the
/// thread**, so the reporter's `Drop` marks the link down when this thread
/// ends for any reason, unwinding included.
pub fn spawn(
    ws_url: String,
    rpc_url: String,
    quote_authority: Pubkey,
    liveness: Option<LivenessReporter<Record>>,
) -> Option<ChannelSource<Fill>> {
    let (source, tx) = ChannelSource::new(FILLS_FEED, FILL_BUFFER);
    let spawned = std::thread::Builder::new()
        .name("maker-bot-fills".into())
        .spawn(move || {
            let rpc = crate::chain::rpc(&rpc_url);
            // Rebound only to obtain the `mut` that `run`'s `&mut` needs. The
            // reporter's drop-at-closure-end — and so its down report — comes
            // from the `move` closure owning it, not from this binding.
            let mut liveness = liveness;
            run(&ws_url, &rpc, &quote_authority, &tx, &mut liveness);
        });
    match spawned {
        Ok(_) => Some(source),
        Err(e) => {
            eprintln!(
                "[fills] could not spawn subscription thread: {e}; using inventory-diff fallback"
            );
            None
        }
    }
}

/// Subscribe, forward fills, and reconnect on websocket drop until the tick's
/// receiver is gone (bot shutting down).
///
/// Every exit from `subscribe_and_forward` is a transport transition, and each
/// is reported: a close is a plain `down` (a socket the venue closed is a
/// state, not an error), while a subscribe or stream failure carries its
/// reason. The shutdown path reports `down` too — otherwise a cleanly stopped
/// bot would leave a row reading `up` for the next operator to read as live.
fn run(
    ws_url: &str,
    rpc: &RpcClient,
    quote_authority: &Pubkey,
    tx: &Sender<Fill>,
    liveness: &mut Option<LivenessReporter<Record>>,
) {
    loop {
        match subscribe_and_forward(ws_url, rpc, quote_authority, tx, liveness) {
            // Receiver dropped — bot is shutting down.
            Ok(true) => {
                if let Some(reporter) = liveness.as_mut() {
                    reporter.down();
                }
                return;
            }
            Ok(false) => {
                if let Some(reporter) = liveness.as_mut() {
                    reporter.down();
                }
                eprintln!("[fills] websocket closed; reconnecting in {RECONNECT_DELAY:?}")
            }
            Err(e) => {
                if let Some(reporter) = liveness.as_mut() {
                    reporter.failed(&e);
                }
                eprintln!("[fills] subscription error: {e}; reconnecting in {RECONNECT_DELAY:?}")
            }
        }
        std::thread::sleep(RECONNECT_DELAY);
        // Retry a transition the telemetry channel refused, once per cycle.
        // Free when there is nothing outstanding.
        if let Some(reporter) = liveness.as_mut() {
            reporter.reassert();
        }
    }
}

/// Render an endpoint as scheme and host only, for text that gets persisted or
/// logged.
///
/// **The websocket URL must never be rendered whole on this path.** It is
/// derived from the operator's `rpc_url`, so it carries whatever credential
/// shape that endpoint uses, and a subscribe failure's text now reaches
/// `push_health.last_error` — a column the read-only dashboard role can
/// `SELECT` and an operations panel renders verbatim.
///
/// The reduction itself is the framework's [`redact_to_origin`], which is also
/// what `LivenessReporter::failed` applies to the **whole** rendered error —
/// including the wrapped client's own `Display`, which is where a transport
/// error re-embeds the URL it could not reach. This wrapper exists only to add
/// the stricter behavior a bare endpoint deserves: a string that is not a URL
/// at all yields a placeholder rather than being passed through, so a malformed
/// endpoint cannot leak by falling out of the parsing.
///
/// Keeping one parser matters more than the two lines it saves — a second copy
/// is how the two paths drift into disagreeing about what an authority is.
fn endpoint_label(ws_url: &str) -> String {
    if !ws_url.contains("://") {
        return "<endpoint>".to_string();
    }
    redact_to_origin(ws_url, MAX_ERROR_CHARS)
}

/// Open one logs subscription and forward attributed fills until it closes.
/// Returns `Ok(true)` if the tick's receiver was dropped (stop), `Ok(false)`
/// if the websocket closed (reconnect).
fn subscribe_and_forward(
    ws_url: &str,
    rpc: &RpcClient,
    quote_authority: &Pubkey,
    tx: &Sender<Fill>,
    liveness: &mut Option<LivenessReporter<Record>>,
) -> Result<bool> {
    let (_subscription, notifications) = PubsubClient::logs_subscribe(
        ws_url,
        RpcTransactionLogsFilter::Mentions(vec![DROPSET_ID.to_string()]),
        RpcTransactionLogsConfig {
            commitment: Some(CommitmentConfig::confirmed()),
        },
    )
    .map_err(|e| anyhow!("logs_subscribe {}: {e}", endpoint_label(ws_url)))?;
    println!(
        "[fills] subscribed to {DROPSET_ID} logs at {}",
        endpoint_label(ws_url)
    );
    // Reported here, on the subscribe — deliberately not on the first
    // notification. "Able to deliver" is the health signal; "did deliver"
    // is a market fact, and conflating them is why the poll seam cannot
    // report a push source at all.
    if let Some(reporter) = liveness.as_mut() {
        reporter.up();
    }

    loop {
        let notification = match notifications.recv_timeout(REASSERT_INTERVAL) {
            Ok(notification) => notification,
            // A quiet interval is this source's healthy state, so a timeout is
            // not an event and must never be treated as one. It is only an
            // opportunity to retry a liveness report the telemetry channel
            // refused earlier — without it, a market with no fills for hours
            // would keep a dropped transition undelivered for just as long.
            //
            // Matched by predicate rather than by variant: this receiver is
            // `crossbeam_channel`'s (the pubsub client's, not `std`'s), and
            // naming its error type would mean taking a direct dependency on
            // that crate to write one arm.
            Err(e) if e.is_timeout() => {
                if let Some(reporter) = liveness.as_mut() {
                    reporter.reassert();
                }
                continue;
            }
            // Disconnected — the only other case: the notification channel
            // closed, which means the websocket dropped.
            Err(_) => return Ok(false),
        };
        let logs = notification.value;
        // A failed transaction commits no fills — its events are rolled back.
        if logs.err.is_some() {
            continue;
        }
        let signature = match Signature::from_str(&logs.signature) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[fills] could not parse signature {}: {e}", logs.signature);
                continue;
            }
        };
        match decode_fills(rpc, &signature, quote_authority) {
            Ok(fills) => {
                for fill in fills {
                    // `blocking_send` blocks this dedicated thread if the
                    // channel is full, and errors only once the runner's
                    // receiver is gone — the bot is shutting down, so stop.
                    if tx.blocking_send(fill).is_err() {
                        return Ok(true);
                    }
                }
            }
            Err(e) => eprintln!("[fills] decode {signature}: {e}"),
        }
    }
}

/// Fetch the transaction and decode every `FillEvent` inner instruction that
/// our program emitted against our vault, attributing by `quote_authority`.
///
/// Each event inner instruction's emitting program is verified by resolving
/// its `program_id_index` against the transaction's full account-key list and
/// requiring it to be `DROPSET_ID` — without it, a third party could craft a
/// `FillEvent`-shaped inner instruction from another program carrying our
/// `quote_authority` (the tag and discriminator are both public). If the
/// account-key list can't be resolved (the transaction won't decode, or a
/// loaded lookup-table address won't parse), no fill is attributed for the
/// transaction and the per-tick vault reconcile in `tasks.rs` is the fallback.
///
/// The loaded addresses are load-bearing, not belt-and-braces: when a swap
/// reaches the program through a CPI whose program account is supplied by an
/// address-lookup table (an aggregator routing into it, say), `DROPSET_ID`
/// sits in the *loaded* segment, so the self-CPI's `program_id_index` points
/// past the static keys. A static-keys-only check would fail to resolve it and
/// wrongly drop a legitimate fill — hence we build the full list or bail.
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

    let tx = confirmed.transaction;
    let Some(meta) = tx.meta else {
        return Ok(Vec::new());
    };
    let OptionSerializer::Some(inner_sets) = meta.inner_instructions else {
        return Ok(Vec::new());
    };

    // Resolve the full account-key list so each event's emitting program can
    // be checked before its bytes are trusted. Bail to the inventory-diff
    // fallback (rather than accept an unverified emitter) if it can't be built.
    let Some(decoded) = tx.transaction.decode() else {
        eprintln!("[fills] {signature}: undecodable transaction; using inventory-diff fallback");
        return Ok(Vec::new());
    };
    let Some(account_keys) = full_account_keys(
        decoded.message.static_account_keys(),
        &meta.loaded_addresses,
    ) else {
        eprintln!(
            "[fills] {signature}: unresolvable loaded addresses; using inventory-diff fallback"
        );
        return Ok(Vec::new());
    };

    Ok(attribute_fills(
        &inner_sets,
        &account_keys,
        signature,
        quote_authority,
    ))
}

/// Walk the inner-instruction sets and collect every `FillEvent` our program
/// emitted against `quote_authority`. Split out from [`decode_fills`] so the
/// emitting-program check is unit-testable without an [`RpcClient`]:
/// `account_keys` is the already-resolved full key list (see
/// [`full_account_keys`]) that `program_id_index` addresses into.
fn attribute_fills(
    inner_sets: &[UiInnerInstructions],
    account_keys: &[Pubkey],
    signature: &Signature,
    quote_authority: &Pubkey,
) -> Vec<Fill> {
    let mut fills = Vec::new();
    for set in inner_sets {
        for instruction in &set.instructions {
            // `emit_cpi!` records events as compiled inner instructions.
            let UiInstruction::Compiled(compiled) = instruction else {
                continue;
            };
            // Only events emitted by our own program count — the tag and
            // discriminator are public, so anyone can forge the bytes, but the
            // emitting program id is what `emit_cpi!`'s self-CPI authenticates.
            if account_keys.get(compiled.program_id_index as usize) != Some(&DROPSET_ID) {
                continue;
            }
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
    fills
}

/// Assemble the transaction's full account-key list in the order an
/// instruction's `program_id_index` addresses: the message's static keys
/// first, then the address-lookup-table loaded addresses (writable, then
/// readonly). Returns `None` if a loaded address won't parse — the caller then
/// can't safely attribute and drops to the inventory-diff fallback rather than
/// trust an unverified emitter.
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

/// Decode one inner-instruction blob as a [`FillEvent`], or `None` if it is a
/// different event / not an event at all. Delegates to the shared SDK codec
/// ([`dropset_sdk::events`]): strip the `EVENT_IX_TAG_LE` self-CPI tag, then
/// decode the `[discriminator][body]` payload and keep only the `Fill` variant.
fn decode_fill_event(data: &[u8]) -> Option<FillEvent> {
    match decode_event_payload(strip_event_tag(data)?)? {
        DropsetEvent::Fill(event) => Some(event),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_sdk::events::EVENT_IX_TAG_LE;
    use sha2::{Digest, Sha256};
    use solana_transaction_status_client_types::UiCompiledInstruction;

    /// `FillEvent`'s 8-byte event discriminator, the anchor name-based scheme
    /// `sha256("event:FillEvent")[..8]` — recomputed here (not a runtime dep)
    /// only to forge the `[tag][discriminator][body]` envelope the decoder
    /// under test consumes. The SDK codec owns the runtime constant.
    fn fill_discriminator() -> [u8; 8] {
        Sha256::digest(b"event:FillEvent")[..8]
            .try_into()
            .expect("sha256 digest is 32 bytes")
    }

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
        data.extend_from_slice(&EVENT_IX_TAG_LE);
        data.extend_from_slice(&fill_discriminator());
        data.extend_from_slice(&borsh::to_vec(event).unwrap());
        data
    }

    /// The borsh body is exactly the on-chain `repr(C)` size — the explicit
    /// padding fields make the two layouts byte-identical (200 bytes:
    /// 4×32-byte keys + u8 + [u8;7] + 2×u32 + 2×u64 + u32 + [u8;4] + 4×u64).
    /// The subscribe URL reaches a column the read-only dashboard role can
    /// read, so every credential shape a hosted endpoint uses has to be gone
    /// before it gets there. The query case is the one the framework's
    /// `sanitize_error` already covers; the other two are exactly what it
    /// documents as surviving it, which is why this reduction exists.
    #[test]
    fn endpoint_label_drops_every_credential_bearing_component() {
        // Query parameter — the shape `sanitize_error` would also have caught.
        assert_eq!(
            endpoint_label("wss://rpc.example/v1/?api-key=SECRET"),
            "wss://rpc.example"
        );
        // Path segment — survives a query-only strip, and is the dominant form
        // at several hosted Solana providers.
        assert_eq!(
            endpoint_label("wss://name.solana-mainnet.example.pro/SECRET/"),
            "wss://name.solana-mainnet.example.pro"
        );
        // Userinfo — likewise survives a query-only strip.
        assert_eq!(
            endpoint_label("wss://user:pass@rpc.example/path"),
            "wss://rpc.example"
        );
        // Fragment, for completeness.
        assert_eq!(
            endpoint_label("wss://rpc.example#SECRET"),
            "wss://rpc.example"
        );
    }

    /// The ordinary localnet endpoint has nothing to strip, so the label is
    /// still the whole useful address — the reduction costs local debugging
    /// nothing.
    #[test]
    fn endpoint_label_keeps_a_bare_host_and_port_intact() {
        assert_eq!(endpoint_label("ws://127.0.0.1:8900"), "ws://127.0.0.1:8900");
        assert_eq!(
            endpoint_label("ws://127.0.0.1:8900/"),
            "ws://127.0.0.1:8900"
        );
    }

    /// A string that is not a URL must not fall through the parsing and be
    /// rendered whole — that would reinstate the leak for a malformed endpoint.
    #[test]
    fn endpoint_label_refuses_to_pass_through_a_non_url() {
        assert_eq!(endpoint_label("not-a-url-with-a-SECRET"), "<endpoint>");
        assert_eq!(endpoint_label(""), "<endpoint>");
    }

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

    /// One inner instruction emitting `event`, claiming to come from the
    /// account at `program_id_index` — the shape an `emit_cpi!` self-CPI lands
    /// as in `getTransaction`'s inner-instruction list (base58 `data`).
    fn compiled_event_ix(program_id_index: u8, event: &FillEvent) -> UiInstruction {
        UiInstruction::Compiled(UiCompiledInstruction {
            program_id_index,
            accounts: Vec::new(),
            data: bs58::encode(wrap(event)).into_string(),
            stack_height: None,
        })
    }

    /// The emitting-program check: a `FillEvent` carrying our `quote_authority`
    /// is attributed only when its inner instruction's `program_id_index`
    /// resolves to `DROPSET_ID` — a byte-identical event emitted by any other
    /// program (the spoof this guards against) is dropped.
    #[test]
    fn attributes_only_events_our_program_emitted() {
        let quote_authority = Pubkey::new_unique();
        let event = sample_event(quote_authority);
        // index 0 is our program, index 1 a foreign program in the same tx.
        let account_keys = vec![DROPSET_ID, Pubkey::new_from_array([9; 32])];
        let inner_sets = vec![UiInnerInstructions {
            index: 0,
            instructions: vec![
                compiled_event_ix(1, &event), // forged: emitted by the foreigner
                compiled_event_ix(0, &event), // genuine: our self-CPI
            ],
        }];
        let fills = attribute_fills(
            &inner_sets,
            &account_keys,
            &Signature::default(),
            &quote_authority,
        );
        assert_eq!(fills.len(), 1, "only the DROPSET_ID-emitted event counts");
        assert_eq!(fills[0].event, event);
    }

    /// An out-of-range `program_id_index` (e.g. an unresolved key) resolves to
    /// no account, so the event is dropped rather than indexing out of bounds.
    #[test]
    fn drops_events_with_an_out_of_range_program_index() {
        let quote_authority = Pubkey::new_unique();
        let event = sample_event(quote_authority);
        let account_keys = vec![DROPSET_ID];
        let inner_sets = vec![UiInnerInstructions {
            index: 0,
            instructions: vec![compiled_event_ix(7, &event)],
        }];
        let fills = attribute_fills(
            &inner_sets,
            &account_keys,
            &Signature::default(),
            &quote_authority,
        );
        assert!(fills.is_empty());
    }

    /// A genuine `DROPSET_ID`-emitted fill against a *different* vault is not
    /// ours — the `quote_authority` filter still applies after the emitter check.
    #[test]
    fn ignores_our_program_events_for_another_vault() {
        let event = sample_event(Pubkey::new_unique()); // some other vault
        let account_keys = vec![DROPSET_ID];
        let inner_sets = vec![UiInnerInstructions {
            index: 0,
            instructions: vec![compiled_event_ix(0, &event)],
        }];
        let fills = attribute_fills(
            &inner_sets,
            &account_keys,
            &Signature::default(),
            &Pubkey::new_unique(), // our vault — not the event's
        );
        assert!(fills.is_empty());
    }

    /// `program_id_index` addresses static keys first, then loaded writable,
    /// then loaded readonly — the order a transaction with a lookup table
    /// composes its account keys.
    #[test]
    fn resolves_keys_static_then_writable_then_readonly() {
        let static_a = Pubkey::new_from_array([10; 32]);
        let static_b = Pubkey::new_from_array([11; 32]);
        let writable = Pubkey::new_from_array([12; 32]);
        let readonly = Pubkey::new_from_array([13; 32]);
        let loaded = OptionSerializer::Some(UiLoadedAddresses {
            writable: vec![writable.to_string()],
            readonly: vec![readonly.to_string()],
        });
        let keys = full_account_keys(&[static_a, static_b], &loaded).expect("all parse");
        assert_eq!(keys, vec![static_a, static_b, writable, readonly]);
    }

    /// With no lookup-table loads the static keys stand alone — both an absent
    /// field and an empty one resolve to just the static list.
    #[test]
    fn resolves_keys_without_loaded_addresses() {
        let static_a = Pubkey::new_from_array([10; 32]);
        for loaded in [
            OptionSerializer::None,
            OptionSerializer::Skip,
            OptionSerializer::Some(UiLoadedAddresses::default()),
        ] {
            let keys = full_account_keys(&[static_a], &loaded).expect("static-only resolves");
            assert_eq!(keys, vec![static_a]);
        }
    }

    /// A loaded address that won't parse means the full list can't be trusted,
    /// so the caller drops to the inventory-diff fallback rather than guess.
    #[test]
    fn malformed_loaded_address_resolves_to_none() {
        let loaded = OptionSerializer::Some(UiLoadedAddresses {
            writable: vec!["not-a-pubkey".to_string()],
            readonly: vec![],
        });
        assert!(full_account_keys(&[Pubkey::new_from_array([10; 32])], &loaded).is_none());
    }
}
