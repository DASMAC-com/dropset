-- One per-tick sample for one market. `ON CONFLICT DO NOTHING` on the
-- `(market, ts)` key absorbs the store sink's at-least-once redelivery, and
-- coalesces two samples that land in the same second (the tick is 5 s, so
-- that is redelivery rather than loss).
INSERT INTO maker_telemetry (
    ts,
    market,
    market_pubkey,
    base_decimals,
    quote_decimals,
    fair,
    reference,
    last_set_price,
    on_chain_reference,
    best_bid,
    best_ask,
    skew_bps,
    anchor,
    regime,
    health,
    degraded,
    uncertain,
    basis,
    basis_breach,
    usdc_breach,
    action,
    halt_reason,
    profile_kind,
    base_value_usd,
    quote_value_usd,
    tvl_usd,
    launch_tvl_usd,
    frozen,
    reference_valid,
    tick_error
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
    $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30
)
ON CONFLICT DO NOTHING;
