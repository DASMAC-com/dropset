//! Adapt a fetched transaction's inner instructions into decoded events
//! at their on-chain coordinates, reusing the shared
//! `dropset_sdk::events` codec.
//!
//! The fetched-transaction type is the framework's [`dropset_feeds::RawTx`]:
//! the ingestion framework flattens and base58-decodes a transaction's inner
//! instructions, and decoding what is inside those blobs is the part that
//! stays Dropset-specific. It is referred to by its framework path
//! throughout this crate rather than re-exported here, so there is one name
//! for one type.

use crate::model::{DecodedEvent, EventCoords};
use dropset_feeds::RawTx;
use dropset_sdk::events::{decode_event_payload, emitted_by, full_account_keys, strip_event_tag};
use solana_pubkey::Pubkey;

/// Decode every event-CPI that `program_id` emitted in this transaction,
/// assigning each its `event_ordinal` — its position among the
/// transaction's event-CPI inner instructions, in walk (heap-pop emission)
/// order (interface.md §1).
///
/// Only blobs the indexed program itself emitted are decoded; see
/// [`dropset_sdk::events::emitted_by`] for why that check is what makes an
/// event-CPI trustworthy at all. The ordinal is assigned over every
/// *tagged* blob, before that check, so it stays a function of the
/// transaction rather than of the trust policy or the codec's coverage.
///
/// The ordinal is assigned at the tag-strip step, **before** the
/// discriminator dispatch, so it counts every event-CPI blob this
/// transaction carries rather than only the ones this build recognizes.
/// That keeps a quarter of the frozen PK a function of the transaction
/// alone: were it assigned after the dispatch, extending the codec would
/// renumber events that are already stored, and since both raw inserts use
/// an untargeted `ON CONFLICT DO NOTHING` — which only suppresses a row
/// that actually violates a unique constraint — a renumbered row inserts
/// as a duplicate and gets folded into the take and volume rollups twice.
///
/// The ordinal still counts only *tagged* blobs, not all inner
/// instructions; the geyser path can supply the true inner-instruction
/// index. Rows written under the older recognized-only numbering keep
/// their ordinals (see `docs/indexer.md`).
pub fn decode_tx(tx: &RawTx, program_id: &Pubkey) -> Vec<DecodedEvent> {
    // Resolve the transaction's account keys once. `None` means a loaded
    // address would not parse, so no blob in this transaction can be
    // attributed to any program and every event here fails closed —
    // dropping real events is the safe direction, ingesting forged ones is
    // not.
    let account_keys = full_account_keys(
        &tx.static_account_keys,
        &tx.loaded_writable,
        &tx.loaded_readonly,
    );
    if account_keys.is_none() {
        tracing::warn!(
            signature = %tx.signature,
            "unresolvable account keys; dropping this transaction's events"
        );
    }

    tx.inner_ix_blobs
        .iter()
        .filter_map(|ix| strip_event_tag(&ix.data).map(|payload| (ix, payload)))
        .enumerate()
        .filter_map(|(ordinal, (ix, payload))| {
            // Only events our own program emitted count. The tag and the
            // discriminator are both public, so the emitting program id is
            // the only part of an event-CPI that `emit_cpi!`'s self-CPI
            // authenticates — and `getSignaturesForAddress` is
            // address-indexed, so a transaction that merely *references*
            // the program is polled and hydrated. Without this a foreign
            // program's `emit_cpi!` of a Dropset-shaped payload would flow
            // into `fill_events`, the take rollups and `/v1`.
            let keys = account_keys.as_deref()?;
            if !emitted_by(keys, ix.program_id_index, program_id) {
                tracing::debug!(
                    signature = %tx.signature,
                    ordinal,
                    "event-CPI blob not emitted by the indexed program; dropped"
                );
                return None;
            }
            decode_event_payload(payload).map(|event| (ordinal, event))
        })
        .map(|(ordinal, event)| DecodedEvent {
            coords: EventCoords {
                slot: tx.slot,
                txn_index: tx.txn_index,
                signature: tx.signature.clone(),
                event_ordinal: ordinal as i64,
                block_time: tx.block_time,
            },
            event,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_feeds::InnerIx;
    use dropset_sdk::events::{DropsetEvent, EVENT_IX_TAG_LE};
    use dropset_sdk::types::FillEvent;

    /// The program these tests index, at account-key index 0.
    fn ours() -> Pubkey {
        Pubkey::new_from_array([1; 32])
    }

    /// A foreign program sharing the transaction, at account-key index 1.
    fn foreign() -> Pubkey {
        Pubkey::new_from_array([9; 32])
    }

    /// A transaction whose account keys resolve index 0 to [`ours`] and
    /// index 1 to [`foreign`].
    fn raw_tx(blobs: Vec<InnerIx>) -> RawTx {
        RawTx {
            slot: 42,
            txn_index: 0,
            signature: "sig".into(),
            block_time: Some(1),
            inner_ix_blobs: blobs,
            static_account_keys: vec![ours(), foreign()],
            loaded_writable: Vec::new(),
            loaded_readonly: Vec::new(),
        }
    }

    /// One inner instruction attributed to `program_id_index`.
    fn from_program(program_id_index: u8, data: Vec<u8>) -> InnerIx {
        InnerIx {
            program_id_index,
            data,
        }
    }

    fn tagged_fill(side: u8) -> Vec<u8> {
        let fill = FillEvent {
            market: solana_pubkey::Pubkey::new_unique(),
            taker: solana_pubkey::Pubkey::new_unique(),
            leader: solana_pubkey::Pubkey::new_unique(),
            quote_authority: solana_pubkey::Pubkey::new_unique(),
            side,
            pad: [0; 7],
            sector_idx: 1,
            level_idx: 2,
            fill_base: 100,
            fill_quote: 200,
            fill_price: 1,
            pad2: [0; 4],
            base_atoms_after: 0,
            quote_atoms_after: 0,
            nonce_after: 0,
            taker_fee_atoms: 0,
        };
        // [tag][discriminator][borsh body]
        let mut data = EVENT_IX_TAG_LE.to_vec();
        data.extend_from_slice(&[13, 89, 41, 228, 105, 178, 45, 112]);
        borsh::to_writer(&mut data, &fill).unwrap();
        data
    }

    /// A blob carrying the event tag but a discriminator this build does
    /// not decode still consumes an ordinal, so the ordinals of the events
    /// around it do not shift when the codec is later extended to decode
    /// it. Without this the frozen PK would depend on codec coverage, and
    /// a re-ingest across a coverage change would double-count takes.
    #[test]
    fn undecodable_tagged_blob_still_consumes_an_ordinal() {
        let mut unknown = EVENT_IX_TAG_LE.to_vec();
        unknown.extend_from_slice(&[9u8; 8]); // no such discriminator
        unknown.extend_from_slice(&[0u8; 16]);

        let tx = raw_tx(vec![
            from_program(0, tagged_fill(0)),
            from_program(0, unknown),
            from_program(0, tagged_fill(1)),
        ]);
        let decoded = decode_tx(&tx, &ours());
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].coords.event_ordinal, 0);
        // 1 is consumed by the undecodable blob between them.
        assert_eq!(decoded[1].coords.event_ordinal, 2);
    }

    #[test]
    fn assigns_sequential_ordinals_and_skips_non_events() {
        let tx = raw_tx(vec![
            from_program(0, vec![0xaa, 0xbb]), // not an event — skipped
            from_program(0, tagged_fill(0)),
            from_program(0, tagged_fill(1)),
        ]);
        let decoded = decode_tx(&tx, &ours());
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].coords.event_ordinal, 0);
        assert_eq!(decoded[1].coords.event_ordinal, 1);
        assert_eq!(decoded[0].coords.slot, 42);
        assert!(matches!(decoded[0].event, DropsetEvent::Fill(_)));
    }

    /// **The spoof this check exists for.** A byte-identical, perfectly
    /// well-formed `FillEvent` emitted by a foreign program in a
    /// transaction that also references ours must not be indexed: the tag
    /// is an anchor-wide constant and the discriminator is a public hash,
    /// so the emitter is the only thing that distinguishes the forgery.
    #[test]
    fn drops_a_fill_emitted_by_a_foreign_program() {
        let tx = raw_tx(vec![
            from_program(1, tagged_fill(0)), // forged by the foreigner
            from_program(0, tagged_fill(1)), // genuine, our self-CPI
        ]);
        let decoded = decode_tx(&tx, &ours());
        assert_eq!(decoded.len(), 1, "only our own event is indexed");
        // The forgery still consumed ordinal 0: the ordinal is a function
        // of the transaction, not of the trust policy.
        assert_eq!(decoded[0].coords.event_ordinal, 1);
    }

    /// A transaction composed entirely of forged events yields nothing —
    /// the fabricated-fill path onto `/v1` is closed, not merely reordered.
    #[test]
    fn drops_every_event_when_none_are_ours() {
        let tx = raw_tx(vec![
            from_program(1, tagged_fill(0)),
            from_program(1, tagged_fill(1)),
        ]);
        assert!(decode_tx(&tx, &ours()).is_empty());
    }

    /// An out-of-range `program_id_index` resolves to no account, so the
    /// event fails closed rather than indexing out of bounds.
    #[test]
    fn drops_events_with_an_out_of_range_program_index() {
        let tx = raw_tx(vec![from_program(7, tagged_fill(0))]);
        assert!(decode_tx(&tx, &ours()).is_empty());
    }

    /// A loaded address that will not parse means no blob in the
    /// transaction can be attributed, so every event is dropped rather
    /// than trusted against a partial key list.
    #[test]
    fn drops_every_event_when_account_keys_are_unresolvable() {
        let mut tx = raw_tx(vec![from_program(0, tagged_fill(0))]);
        tx.loaded_writable = vec!["not-a-pubkey".to_string()];
        assert!(decode_tx(&tx, &ours()).is_empty());
    }

    /// The check is against the *configured* program, not a compiled-in
    /// constant — a localnet deployment indexes its own program id.
    #[test]
    fn trusts_the_configured_program_not_a_hardcoded_one() {
        let tx = raw_tx(vec![from_program(1, tagged_fill(0))]);
        // Indexing the foreign program instead makes that same blob ours.
        assert_eq!(decode_tx(&tx, &foreign()).len(), 1);
    }
}
