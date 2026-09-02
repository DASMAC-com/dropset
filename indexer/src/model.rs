//! Row types, the `/v1` wire shape, and the decoded-event → JSON / column
//! projections the store writes.
//!
//! Pubkeys serialize as base58 strings and `u64` atoms as Postgres NUMERIC
//! (read back as [`Decimal`], serialized as strings so a value above
//! `i64::MAX` keeps full precision).
//!
//! That precision rule binds **both** tiers of this boundary: the typed
//! fill columns get it from `rust_decimal`'s `serde-str`, and the JSONB
//! payload gets it from `wide`. One stated contract honoured in only one
//! of two tiers is the kind of inconsistency a client SDK cannot encode
//! once.

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
        DropsetEvent::SetMaxPlatformFee(x) => Some(pk(&x.market)),
        DropsetEvent::PlatformFee(x) => Some(pk(&x.market)),
        DropsetEvent::SweepResidual(x) => Some(pk(&x.market)),
        DropsetEvent::CloseMarketTreasury(x) => Some(pk(&x.market)),
        DropsetEvent::SetDefaultFeeConfig(_) => None,
        DropsetEvent::SetRegistryDefaults(_) => None,
        // Registry-level: names a fee mint, not a market.
        DropsetEvent::CloseRegistryFeeVault(_) => None,
    }
}

/// A wide integer as a JSON **string**.
///
/// The module contract above says `u64` atoms cross this boundary as
/// strings, and the typed fill tier honours it for real (`Decimal` columns
/// with `rust_decimal`'s `serde-str`). This is how the JSONB tier honours
/// the same rule: interpolating a raw `u64` into `json!` emits a bare JSON
/// number, and any JavaScript consumer — which is what a `/v1` JSON API is
/// for — parses that as a double and silently rounds above 2^53. The
/// stored text would be exact and the client's value wrong, which surfaces
/// as an unreproducible off-by-a-few long after the fact.
///
/// `i64` gets the same treatment: it is equally past 2^53, so
/// `realized_pnl_delta` would round exactly like an atom count.
fn wide(v: impl std::fmt::Display) -> String {
    v.to_string()
}

/// The decoded event as JSON for the `events.payload` column and the
/// `/v1/events` response.
pub fn event_to_json(e: &DropsetEvent) -> Value {
    match e {
        DropsetEvent::Fill(x) => json!({
            "market": pk(&x.market), "taker": pk(&x.taker), "leader": pk(&x.leader),
            "quote_authority": pk(&x.quote_authority), "side": x.side,
            "sector_idx": x.sector_idx, "level_idx": x.level_idx,
            "fill_base": wide(x.fill_base), "fill_quote": wide(x.fill_quote),
            "fill_price": x.fill_price,
            "base_atoms_after": wide(x.base_atoms_after),
            "quote_atoms_after": wide(x.quote_atoms_after),
            "nonce_after": wide(x.nonce_after),
            "taker_fee_atoms": wide(x.taker_fee_atoms),
        }),
        DropsetEvent::Deposit(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "depositor": pk(&x.depositor),
            "is_leader": x.is_leader, "is_seeding": x.is_seeding,
            "base_in": wide(x.base_in), "quote_in": wide(x.quote_in),
            "shares_out": wide(x.shares_out),
            "total_shares_after": wide(x.total_shares_after),
            "leader_shares_after": wide(x.leader_shares_after),
            "base_atoms_after": wide(x.base_atoms_after),
            "quote_atoms_after": wide(x.quote_atoms_after),
        }),
        DropsetEvent::Withdraw(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "depositor": pk(&x.depositor),
            "is_leader": x.is_leader, "shares_in": wide(x.shares_in),
            "base_out": wide(x.base_out), "quote_out": wide(x.quote_out),
            "total_shares_after": wide(x.total_shares_after),
            "leader_shares_after": wide(x.leader_shares_after),
            "base_atoms_after": wide(x.base_atoms_after),
            "quote_atoms_after": wide(x.quote_atoms_after),
            "realized_pnl_delta": wide(x.realized_pnl_delta),
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
            "market": pk(&x.market), "sector_idx": x.sector_idx,
            "shares_minted": wide(x.shares_minted),
            "leader_shares_after": wide(x.leader_shares_after),
            "total_shares_after": wide(x.total_shares_after),
            "hwm_after": wide(x.hwm_after),
        }),
        DropsetEvent::SetMinLeaderShare(x) => json!({
            "market": pk(&x.market), "sector_idx": x.sector_idx, "min_leader_share": x.min_leader_share,
        }),
        DropsetEvent::SetMarketFeeConfig(x) => json!({
            "market": pk(&x.market), "mint": pk(&x.mint),
            "token_program": pk(&x.token_program), "atoms": wide(x.atoms),
        }),
        DropsetEvent::SetTakerFee(x) => json!({
            "market": pk(&x.market), "taker_fee": x.taker_fee,
        }),
        DropsetEvent::SetDefaultFeeConfig(x) => json!({
            "mint": pk(&x.mint), "token_program": pk(&x.token_program), "atoms": wide(x.atoms),
        }),
        // All three defaults, not just the two the instruction changed:
        // the program emits the full post-update set precisely so an
        // indexer records it, and `default_max_platform_fee` is the
        // ceiling on how much any router may skim from a taker.
        DropsetEvent::SetRegistryDefaults(x) => json!({
            "default_taker_fee": x.default_taker_fee,
            "default_max_platform_fee": x.default_max_platform_fee,
            "default_min_leader_share": x.default_min_leader_share,
        }),
        DropsetEvent::SetMaxPlatformFee(x) => json!({
            "market": pk(&x.market), "max_platform_fee": x.max_platform_fee,
        }),
        DropsetEvent::PlatformFee(x) => json!({
            "market": pk(&x.market), "taker": pk(&x.taker),
            "fee_authority": pk(&x.fee_authority), "mint": pk(&x.mint),
            "atoms": wide(x.atoms), "platform_fee_bps": x.platform_fee_bps,
        }),
        DropsetEvent::SweepResidual(x) => json!({
            "market": pk(&x.market), "mint": pk(&x.mint),
            "token_recipient": pk(&x.token_recipient),
            "treasury_amount": wide(x.treasury_amount), "vault_sum": wide(x.vault_sum),
            "accrued_fee": wide(x.accrued_fee), "swept": wide(x.swept),
        }),
        DropsetEvent::CloseMarketTreasury(x) => json!({
            "market": pk(&x.market), "mint": pk(&x.mint), "is_base": x.is_base,
            "token_recipient": pk(&x.token_recipient),
            "rent_recipient": pk(&x.rent_recipient),
            "drained": wide(x.drained), "accrued_fee": wide(x.accrued_fee),
        }),
        DropsetEvent::CloseRegistryFeeVault(x) => json!({
            "fee_mint": pk(&x.fee_mint), "token_recipient": pk(&x.token_recipient),
            "rent_recipient": pk(&x.rent_recipient), "collected": wide(x.collected),
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

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_sdk::events::try_decode_event_payload;

    const IDL: &str = include_str!("../../sdk/idl/dropset.json");

    /// The wire size of one IDL field type.
    ///
    /// `Price` is special-cased because the IDL records it as a struct with
    /// no fields: it is a `u32` alias whose anchor `IdlType` derive is
    /// gated off in the off-chain build, so it emits as raw `u32` bits
    /// (see `sdk/rs/src/generated/types/price.rs`).
    fn field_size(ty: &Value) -> usize {
        if let Some(name) = ty.as_str() {
            return match name {
                "pubkey" => 32,
                "bool" | "u8" | "i8" => 1,
                "u16" | "i16" => 2,
                "u32" | "i32" => 4,
                "u64" | "i64" => 8,
                other => panic!("unhandled IDL field type {other} — teach field_size about it"),
            };
        }
        if let Some(array) = ty.get("array").and_then(|a| a.as_array()) {
            let count = array[1].as_u64().expect("array length") as usize;
            return count * field_size(&array[0]);
        }
        if ty.get("defined").and_then(|d| d["name"].as_str()) == Some("Price") {
            return 4;
        }
        panic!("unhandled IDL field type {ty} — teach field_size about it");
    }

    /// **Field completeness, per variant, against the IDL.**
    ///
    /// For every event the IDL declares: decode a zero-filled body of the
    /// size the IDL implies, project it, and require the JSON object to
    /// carry exactly the event's non-padding fields.
    ///
    /// This is the guard whose absence let `SetRegistryDefaultsEvent` reach
    /// production missing `default_max_platform_fee`. A dropped key is not
    /// a compile error, because `json!` reads fields one at a time — so
    /// nothing failed. Executing the projection is not enough either: the
    /// Postgres test already drove a `DepositEvent` through it and passed.
    /// What was missing was an assertion about *completeness*.
    ///
    /// Zero-filling is sound because every event body is fixed-size — no
    /// `Vec` or `String` fields — and a zero byte is a valid `bool`,
    /// pubkey and integer. That also makes this a size check: if the codec
    /// and the IDL disagree on a field's width, the decode fails or leaves
    /// trailing bytes.
    #[test]
    fn every_event_payload_carries_every_field_the_idl_declares() {
        let idl: Value = serde_json::from_str(IDL).expect("the generated IDL parses");
        let types = idl["types"].as_array().expect("IDL types");
        let events = idl["events"].as_array().expect("IDL events");
        assert!(!events.is_empty(), "no events read — is the path right?");

        for event in events {
            let name = event["name"].as_str().expect("event name");
            let discriminator: Vec<u8> = event["discriminator"]
                .as_array()
                .expect("event discriminator")
                .iter()
                .map(|b| b.as_u64().expect("a discriminator byte") as u8)
                .collect();

            let fields = types
                .iter()
                .find(|t| t["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("{name} has no type definition"))["type"]["fields"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} has no fields"));

            let body_len: usize = fields.iter().map(|f| field_size(&f["type"])).sum();
            let mut payload = discriminator;
            payload.extend(std::iter::repeat_n(0u8, body_len));

            let decoded = try_decode_event_payload(&payload)
                .unwrap_or_else(|e| panic!("{name} did not decode from a zeroed body: {e:?}"));
            assert_eq!(decoded.name(), name, "variant name disagrees with the IDL");

            let payload_json = event_to_json(&decoded);
            let object = payload_json
                .as_object()
                .unwrap_or_else(|| panic!("{name} did not project to an object"));

            // Padding is a layout artifact, not data — the IDL names it
            // with a leading underscore.
            let expected: Vec<&str> = fields
                .iter()
                .map(|f| f["name"].as_str().expect("field name"))
                .filter(|f| !f.starts_with('_'))
                .collect();

            for field in &expected {
                assert!(
                    object.contains_key(*field),
                    "{name}.{field} is decoded but dropped from the JSON payload"
                );
            }

            // Value TYPE, not just key presence. Key presence alone leaves
            // the precision rule pinned on whichever field happens to have
            // a hand-written assertion — reverting `wide()` at any other
            // call site would keep this test green. Driving it off the
            // IDL's own widths pins every variant at once, and catches a
            // FUTURE wide field added without `wide()`, which is the exact
            // bug class this rule exists for.
            //
            // The body is zero-filled, so every wide value serializes as
            // "0" and `is_string()` holds without special-casing.
            for field in fields {
                let field_name = field["name"].as_str().expect("field name");
                if field_name.starts_with('_') {
                    continue;
                }
                let Some(width) = field["type"].as_str() else {
                    continue; // arrays and defined types carry no width rule
                };
                let value = &object[field_name];
                match width {
                    "u64" | "i64" => assert!(
                        value.is_string(),
                        "{name}.{field_name} is a {width} and must cross as a JSON \
                         string — a bare number rounds above 2^53 in any JavaScript \
                         consumer of /v1"
                    ),
                    "u8" | "u16" | "u32" | "i8" | "i16" | "i32" => assert!(
                        value.is_u64() || value.is_i64(),
                        "{name}.{field_name} is a bounded {width}; quoting it only \
                         makes a client parse what it could have read"
                    ),
                    "bool" => assert!(value.is_boolean(), "{name}.{field_name}"),
                    "pubkey" => assert!(value.is_string(), "{name}.{field_name}"),
                    other => panic!("unhandled IDL width {other} on {name}.{field_name}"),
                }
            }
            assert_eq!(
                object.len(),
                expected.len(),
                "{name} projects keys the IDL does not declare: {:?}",
                object
                    .keys()
                    .filter(|k| !expected.contains(&k.as_str()))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Wide integers cross as JSON strings, so a value past 2^53 survives
    /// a JavaScript consumer. Bounded fields (`u8`/`u16`/`u32`) stay
    /// numbers — they cannot overflow a double and quoting them would just
    /// make a client parse what it could have read.
    #[test]
    fn wide_integers_are_strings_and_narrow_ones_are_not() {
        let event =
            DropsetEvent::SetRegistryDefaults(dropset_sdk::types::SetRegistryDefaultsEvent {
                default_taker_fee: 30,
                default_max_platform_fee: 50,
                default_min_leader_share: 1_000,
            });
        let json = event_to_json(&event);
        assert!(
            json["default_max_platform_fee"].is_u64(),
            "u16 stays a number"
        );

        let deposit = DropsetEvent::Deposit(dropset_sdk::types::DepositEvent {
            market: Pubkey::new_unique(),
            sector_idx: 1,
            depositor: Pubkey::new_unique(),
            is_leader: true,
            is_seeding: false,
            base_in: u64::MAX,
            quote_in: 20,
            shares_out: 30,
            total_shares_after: 30,
            leader_shares_after: 30,
            base_atoms_after: 10,
            quote_atoms_after: 20,
        });
        let json = event_to_json(&deposit);
        assert_eq!(
            json["base_in"],
            Value::String(u64::MAX.to_string()),
            "a u64 past 2^53 must not be a bare JSON number"
        );
    }
}
