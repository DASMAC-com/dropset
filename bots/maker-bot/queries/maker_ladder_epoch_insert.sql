-- One level of one side of the ladder the bot just armed. Written once per
-- re-arm rather than once per tick — see `0018_maker_ladder_epoch` for why the
-- per-tick shape was rejected and how a consumer derives per-tick prices from
-- this plus `maker_telemetry.on_chain_reference`.
--
-- Idempotent on `(market, armed_at, side, level_idx)` — named here because
-- `ON CONFLICT DO NOTHING` carries no target, so the statement itself does not
-- say what the key is.
--
-- Inside one process's tick loop a conflict is unreachable: the cold path sends
-- at most one profile per market per cycle and every arm path returns straight
-- after it, so two re-arms cannot share a wall-clock second at the 5 s tick.
-- What this clause actually absorbs is the residue outside that argument — a
-- process restart that re-arms inside the same second as the previous
-- process's last arm, and a backward clock step, `armed_at` being wall time.
-- Be precise about what that costs, because it is more than one row. Every
-- row of one epoch shares an `armed_at`, so a conflict drops the whole epoch
-- and leaves the PREVIOUS shape as the latest one on record — a consumer then
-- keeps reading the old ladder until the next re-arm, which the daily
-- heartbeat bounds at a day. That over-states resting liquidity, the one
-- direction a consumer of this table most wants to avoid.
--
-- It is still the right trade, twice over. An aborted transaction would take
-- the tick's sample and legs down with it, losing strictly more. And
-- `DO UPDATE` would not help: a shorter new ladder cannot evict the old
-- epoch's surplus deep rows, since they carry the same `armed_at` and no
-- longer appear in the incoming set at all.
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
