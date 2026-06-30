//! Adapt a fetched transaction's inner instructions into decoded events
//! at their on-chain coordinates, reusing the shared
//! `dropset_sdk::events` codec.

use crate::model::{DecodedEvent, EventCoords};
use dropset_sdk::events::{decode_event_payload, strip_event_tag};

/// A fetched transaction reduced to what the indexer needs: its
/// coordinates and the raw `data` of each inner instruction, in order.
#[derive(Clone, Debug)]
pub struct RawTx {
    pub slot: i64,
    pub txn_index: i64,
    pub signature: String,
    pub block_time: Option<i64>,
    pub inner_ix_blobs: Vec<Vec<u8>>,
}

/// Decode every Dropset event-CPI from a transaction, assigning each its
/// `event_ordinal` — its position among the transaction's event-CPI inner
/// instructions, in walk (heap-pop emission) order (interface.md §1).
///
/// The prototype's ordinal counts only the recognized event-CPIs, not all
/// inner instructions; the relative order is what the PK needs for
/// dedup, and the geyser path can supply the true inner-instruction index.
pub fn decode_tx(tx: &RawTx) -> Vec<DecodedEvent> {
    tx.inner_ix_blobs
        .iter()
        .filter_map(|d| strip_event_tag(d))
        .filter_map(decode_event_payload)
        .enumerate()
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
    use dropset_sdk::events::{DropsetEvent, EVENT_IX_TAG_LE};
    use dropset_sdk::types::FillEvent;

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

    #[test]
    fn assigns_sequential_ordinals_and_skips_non_events() {
        let tx = RawTx {
            slot: 42,
            txn_index: 0,
            signature: "sig".into(),
            block_time: Some(1),
            inner_ix_blobs: vec![
                vec![0xaa, 0xbb], // not an event — skipped
                tagged_fill(0),
                tagged_fill(1),
            ],
        };
        let decoded = decode_tx(&tx);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].coords.event_ordinal, 0);
        assert_eq!(decoded[1].coords.event_ordinal, 1);
        assert_eq!(decoded[0].coords.slot, 42);
        assert!(matches!(decoded[0].event, DropsetEvent::Fill(_)));
    }
}
