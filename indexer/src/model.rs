//! Row types, the `/v1` wire shape, and the decoded-event → JSON / column
//! projections the store writes.
//!
//! Pubkeys serialize as base58 strings and `u64` atoms as Postgres NUMERIC
//! (read back as [`Decimal`], serialized as strings so a value above
//! `i64::MAX` keeps full precision).

use dropset_sdk::events::DropsetEvent;
use dropset_sdk::types::FillEvent;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Value};
use solana_pubkey::Pubkey;

/// The frozen event primary key (interface.md §1), plus the block time
/// carried alongside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventCoords {
    pub slot: i64,
    pub txn_index: i64,
    pub signature: String,
    pub event_ordinal: i64,
    pub block_time: Option<i64>,
}

/// A decoded event together with its on-chain coordinates.
#[derive(Clone, Debug)]
pub struct DecodedEvent {
    pub coords: EventCoords,
    pub event: DropsetEvent,
}

fn pk(p: &Pubkey) -> String {
    p.to_string()
}

/// The market this event pertains to, or `None` for the registry-level
/// admin events that name no market.
pub fn event_market(e: &DropsetEvent) -> Option<String> {
    match e {
        DropsetEvent::Fill(x) => Some(pk(&x.market)),
        DropsetEvent::Deposit(x) => Some(pk(&x.market)),
        DropsetEvent::Withdraw(x) => Some(pk(&x.market)),
        DropsetEvent::CreateVault(x) => Some(pk(&x.market)),
        DropsetEvent::CloseVault(x) => Some(pk(&x.market)),
        DropsetEvent::FreezeVault(x) => Some(pk(&x.market)),
        DropsetEvent::Realize(x) => Some(pk(&x.market)),
        DropsetEvent::SetMinLeaderShare(x) => Some(pk(&x.market)),
        DropsetEvent::SetMarketFeeConfig(x) => Some(pk(&x.market)),
        DropsetEvent::SetTakerFee(x) => Some(pk(&x.market)),
        DropsetEvent::SetDefaultFeeConfig(_) => None,
        DropsetEvent::SetRegistryDefaults(_) => None,
    }
}

/// The decoded event as JSON for the `events.payload` column and the
/// `/v1/events` response.
pub fn event_to_json(e: &DropsetEvent) -> Value {
    match e {
        DropsetEvent::Fill(x) => json!({
            "market": pk(&x.market), "taker": pk(&x.taker), "leader": pk(&x.leader),
            "quote_authority": pk(&x.quote_authority), "side": x.side,
            "sector_idx": x.sector_idx, "level_idx": x.level_idx,
            "fill_base": x.fill_base, "fill_quote": x.fill_quote, "fill_price": x.fill_price,
            "base_atoms_after": x.base_atoms_after, "quote_atoms_after": x.quote_atoms_after,
            "nonce_after": x.nonce_after, "taker_fee_atoms": x.taker_fee_atoms,
        }),
        DropsetEvent::Deposit(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "depositor": pk(&x.depositor),
            "is_leader": x.is_leader, "is_seeding": x.is_seeding,
            "base_in": x.base_in, "quote_in": x.quote_in, "shares_out": x.shares_out,
            "total_shares_after": x.total_shares_after, "leader_shares_after": x.leader_shares_after,
            "base_atoms_after": x.base_atoms_after, "quote_atoms_after": x.quote_atoms_after,
        }),
        DropsetEvent::Withdraw(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "depositor": pk(&x.depositor),
            "is_leader": x.is_leader, "shares_in": x.shares_in,
            "base_out": x.base_out, "quote_out": x.quote_out,
            "total_shares_after": x.total_shares_after, "leader_shares_after": x.leader_shares_after,
            "base_atoms_after": x.base_atoms_after, "quote_atoms_after": x.quote_atoms_after,
            "realized_pnl_delta": x.realized_pnl_delta,
        }),
        DropsetEvent::CreateVault(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "leader": pk(&x.leader),
            "quote_authority": pk(&x.quote_authority), "perf_fee_rate": x.perf_fee_rate,
            "min_leader_share": x.min_leader_share, "allow_outside_depositors": x.allow_outside_depositors,
        }),
        DropsetEvent::CloseVault(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "leader": pk(&x.leader),
            "active_count_after": x.active_count_after,
        }),
        DropsetEvent::FreezeVault(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "leader": pk(&x.leader),
        }),
        DropsetEvent::Realize(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "shares_minted": x.shares_minted,
            "leader_shares_after": x.leader_shares_after, "total_shares_after": x.total_shares_after,
            "hwm_after": x.hwm_after,
        }),
        DropsetEvent::SetMinLeaderShare(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "min_leader_share": x.min_leader_share,
        }),
        DropsetEvent::SetMarketFeeConfig(x) => json!({
            "market": pk(&x.market), "mint": pk(&x.mint),
            "token_program": pk(&x.token_program), "atoms": x.atoms,
        }),
        DropsetEvent::SetTakerFee(x) => json!({
            "market": pk(&x.market), "taker_fee": x.taker_fee,
        }),
        DropsetEvent::SetDefaultFeeConfig(x) => json!({
            "mint": pk(&x.mint), "token_program": pk(&x.token_program), "atoms": x.atoms,
        }),
        DropsetEvent::SetRegistryDefaults(x) => json!({
            "default_taker_fee": x.default_taker_fee, "default_min_leader_share": x.default_min_leader_share,
        }),
    }
}

/// A raw fill leg, typed for the `fill_events` table and the `/v1/fills`
/// response.
#[derive(Clone, Debug, PartialEq, Serialize, sqlx::FromRow)]
pub struct FillRow {
    pub slot: i64,
    pub txn_index: i64,
    pub signature: String,
    pub event_ordinal: i64,
    pub block_time: Option<i64>,
    pub market: String,
    pub taker: String,
    pub leader: String,
    pub quote_authority: String,
    pub side: i16,
    pub sector_idx: i64,
    pub level_idx: i64,
    pub fill_base: Decimal,
    pub fill_quote: Decimal,
    pub fill_price: i64,
    pub base_atoms_after: Decimal,
    pub quote_atoms_after: Decimal,
    pub nonce_after: Decimal,
    pub taker_fee_atoms: Decimal,
}

impl FillRow {
    /// Project a decoded [`FillEvent`] at its coordinates into a row.
    pub fn from_event(coords: &EventCoords, f: &FillEvent) -> Self {
        Self {
            slot: coords.slot,
            txn_index: coords.txn_index,
            signature: coords.signature.clone(),
            event_ordinal: coords.event_ordinal,
            block_time: coords.block_time,
            market: pk(&f.market),
            taker: pk(&f.taker),
            leader: pk(&f.leader),
            quote_authority: pk(&f.quote_authority),
            side: i16::from(f.side),
            sector_idx: i64::from(f.sector_idx),
            level_idx: i64::from(f.level_idx),
            fill_base: Decimal::from(f.fill_base),
            fill_quote: Decimal::from(f.fill_quote),
            fill_price: i64::from(f.fill_price),
            base_atoms_after: Decimal::from(f.base_atoms_after),
            quote_atoms_after: Decimal::from(f.quote_atoms_after),
            nonce_after: Decimal::from(f.nonce_after),
            taker_fee_atoms: Decimal::from(f.taker_fee_atoms),
        }
    }
}

/// One take: the `(signature, txn_index)` group of fill legs — the
/// take-level view interface.md §1 calls "derived, not emitted".
#[derive(Clone, Debug, PartialEq, Serialize, sqlx::FromRow)]
pub struct Take {
    pub signature: String,
    pub txn_index: i64,
    pub slot: i64,
    pub block_time: Option<i64>,
    pub market: String,
    pub taker: String,
    pub side: i16,
    pub leg_count: i32,
    pub total_fill_base: Decimal,
    pub total_fill_quote: Decimal,
    pub total_taker_fee: Decimal,
    /// `total_fill_quote / total_fill_base`, in atoms (decimal-scaling is a
    /// client concern). `None` when the take filled zero base.
    pub avg_price: Option<f64>,
}

/// Per-market rollup row for `/v1/markets`.
#[derive(Clone, Debug, PartialEq, Serialize, sqlx::FromRow)]
pub struct MarketStatsRow {
    pub market: String,
    pub last_price: Option<f64>,
    pub last_slot: i64,
    pub take_count: i64,
    pub volume_base: Decimal,
    pub volume_quote: Decimal,
    pub volume_base_adjusted: Option<Decimal>,
    pub volume_quote_adjusted: Option<Decimal>,
}
