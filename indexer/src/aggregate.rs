//! The watermarked aggregator: fold per-leg fills into takes and refresh
//! per-market rollups. The grouping is a pure function ([`group_takes`])
//! recomputed from each take's full leg set, so re-folding is idempotent —
//! a watermark that lands mid-take and a replayed slot both converge to the
//! same row (docs/indexer.md §6).

use crate::model::{FillRow, Take};
use crate::store::{Cursor, Store};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet};

/// Group fill legs into takes by `(signature, txn_index)`, summing the
/// per-leg figures into the take-level totals interface.md §1 derives.
pub fn group_takes(fills: &[FillRow]) -> Vec<Take> {
    let mut groups: BTreeMap<(String, i64), Vec<&FillRow>> = BTreeMap::new();
    for f in fills {
        groups
            .entry((f.signature.clone(), f.txn_index))
            .or_default()
            .push(f);
    }
    groups
        .into_iter()
        .map(|((signature, txn_index), legs)| {
            let total_fill_base: Decimal = legs.iter().map(|l| l.fill_base).sum();
            let total_fill_quote: Decimal = legs.iter().map(|l| l.fill_quote).sum();
            let total_taker_fee: Decimal = legs.iter().map(|l| l.taker_fee_atoms).sum();
            let first = legs[0];
            let slot = legs.iter().map(|l| l.slot).max().unwrap_or(first.slot);
            let avg_price = if total_fill_base.is_zero() {
                None
            } else {
                (total_fill_quote / total_fill_base).to_f64()
            };
            Take {
                signature,
                txn_index,
                slot,
                block_time: first.block_time,
                market: first.market.clone(),
                taker: first.taker.clone(),
                side: first.side,
                leg_count: legs.len() as i32,
                total_fill_base,
                total_fill_quote,
                total_taker_fee,
                avg_price,
            }
        })
        .collect()
}

/// One aggregation pass: fold every fill leg past the watermark into its
/// take, refresh the touched markets, and advance the watermark. Returns
/// the number of new legs folded.
pub async fn run_once(store: &Store, batch_limit: i64) -> anyhow::Result<usize> {
    let cursor = store.cursor().await?;
    let new_fills = store.fills_after(&cursor, batch_limit).await?;
    if new_fills.is_empty() {
        return Ok(0);
    }

    let touched: BTreeSet<(String, i64)> = new_fills
        .iter()
        .map(|f| (f.signature.clone(), f.txn_index))
        .collect();
    let mut markets: BTreeSet<String> = BTreeSet::new();
    for (signature, txn_index) in &touched {
        let legs = store.legs_for(signature, *txn_index).await?;
        for take in group_takes(&legs) {
            markets.insert(take.market.clone());
            store.upsert_take(&take).await?;
        }
    }
    for market in &markets {
        store.recompute_market_stats(market).await?;
    }

    let last = new_fills.last().expect("non-empty");
    store
        .set_cursor(&Cursor {
            last_slot: last.slot,
            last_txn_index: last.txn_index,
            last_event_ordinal: last.event_ordinal,
            last_signature: last.signature.clone(),
        })
        .await?;
    Ok(new_fills.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(sig: &str, ordinal: i64, base: i64, quote: i64, fee: i64) -> FillRow {
        FillRow {
            slot: 10,
            txn_index: 0,
            signature: sig.into(),
            event_ordinal: ordinal,
            block_time: Some(5),
            market: "MKT".into(),
            taker: "TKR".into(),
            leader: "LDR".into(),
            quote_authority: "QA".into(),
            side: 0,
            sector_idx: 1,
            level_idx: ordinal,
            fill_base: Decimal::from(base),
            fill_quote: Decimal::from(quote),
            fill_price: 1,
            base_atoms_after: Decimal::ZERO,
            quote_atoms_after: Decimal::ZERO,
            nonce_after: Decimal::ZERO,
            taker_fee_atoms: Decimal::from(fee),
        }
    }

    #[test]
    fn groups_legs_of_one_take_and_sums() {
        let fills = vec![
            leg("a", 0, 100, 400, 1),
            leg("a", 1, 100, 600, 2),
            leg("b", 0, 50, 50, 0),
        ];
        let takes = group_takes(&fills);
        assert_eq!(takes.len(), 2);
        let a = takes.iter().find(|t| t.signature == "a").unwrap();
        assert_eq!(a.leg_count, 2);
        assert_eq!(a.total_fill_base, Decimal::from(200));
        assert_eq!(a.total_fill_quote, Decimal::from(1000));
        assert_eq!(a.total_taker_fee, Decimal::from(3));
        // avg_price = 1000 / 200 = 5.0
        assert_eq!(a.avg_price, Some(5.0));
    }

    #[test]
    fn zero_base_take_has_no_avg_price() {
        let takes = group_takes(&[leg("z", 0, 0, 0, 0)]);
        assert_eq!(takes[0].avg_price, None);
    }
}
