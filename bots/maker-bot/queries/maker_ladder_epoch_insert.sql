-- One level of one side of the ladder the bot just armed. Written once per
-- re-arm rather than once per tick — see `0018_maker_ladder_epoch` for why the
-- per-tick shape was rejected and how a consumer derives per-tick prices from
-- this plus `maker_telemetry.on_chain_reference`.
--
-- Idempotent on the whole key for the same reason as the sample insert: the
-- cold path sends at most one profile per market per cycle, so a conflict needs
-- two re-arms inside one wall-clock second at a 5 s tick. If that assumption
-- ever breaks, silently dropping one telemetry row is the right price — an
-- aborted transaction would take the tick's sample and legs down with it.
INSERT INTO maker_ladder_epoch (
    armed_at,
    market,
    side,
    level_idx,
    offset_ppm,
    size_bps,
    profile_kind
)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT DO NOTHING;
