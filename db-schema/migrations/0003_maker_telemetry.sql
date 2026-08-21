-- cspell:word unfresh
-- Maker operational telemetry: the read side of running a hosted maker
-- (docs/market-making.md §6, docs/data-feeds.md §8).
--
-- Three tables, split by *cadence and key* rather than by subject:
--
--   * `maker_telemetry` — one sample per market per tick. The quoting
--     reference, the composition regime, the kill-switch decision, and the
--     valued inventory, all as of one tick. This is also the heartbeat: a
--     live bot writes a row every tick, so "no row recently" is the
--     dead-bot signal and needs no separate table.
--   * `maker_legs`     — one row per market per feed leg per tick, carrying
--     the leg's value, age, and confidence half-width. Separate from the
--     sample above because a market has N legs, so folding them in would
--     mean either N value columns or one row per leg duplicating every
--     sample column.
--   * `feed_health`    — current status per registered feed source, keyed
--     by `Source::name()` and upserted. One row per source, not a history:
--     the history of what a feed *said* is `maker_legs`; this is whether
--     the poller itself is alive.
--
-- **Why `feed_health` carries no value column.** The obvious design is one
-- table holding "feed id, status, last value, confidence", and it does not
-- fit the code: the framework auto-registers health by `Source::name()`,
-- and the bot's price sources are *venue*-level (`pyth-hermes`, `kraken`,
-- `coingecko`, `frankfurter`) — each yields a map of many instruments in
-- one batch. A single `last_value` on that row would have to pick one
-- instrument arbitrarily and would read as authoritative. So the generic
-- row carries only what the framework actually knows generically (is it
-- polling, when last, with what error), and per-instrument values live in
-- `maker_legs`, where the consumer that resolved the instrument writes
-- them. A dashboard joins the two on the feed name.
--
-- **Time is BIGINT epoch seconds**, matching `cex_prices.bucket_start` and
-- the indexer's `block_time`, and for the same two reasons: it keeps the
-- writers free of a date-library dependency, and it is the column shape
-- Grafana's `$__unixEpochFilter` macro takes as a *bare column name*
-- (wrapping a column in `to_timestamp(...)` inside a macro argument trips
-- the macro's paren-matching — see the note in
-- market-data/grafana/dashboards/market-data.json).
--
-- **The per-tick primary key is `(market, ts)` at one-second resolution**,
-- which is what makes these writes idempotent under the store sink's
-- at-least-once delivery: a re-sent batch lands on `ON CONFLICT DO
-- NOTHING` instead of duplicating a sample. The tick is 5 s
-- (`BotConfig::tick`, the §3 heartbeat), so a one-second key never
-- coalesces two distinct samples; a sub-second tick would, and that is the
-- tradeoff being taken deliberately rather than by accident.
--
-- **On `dropset_ro` and inventory.** 0002 grants the read-only role
-- `SELECT` on every table in `public`, present and future, and its comment
-- warns that position data does not belong there. These tables hold valued
-- inventory, and they stay in `public` anyway — deliberately: Grafana is
-- the intended reader and the inventory panel is the point of the issue
-- this migration serves. What 0002 is guarding against is *credentials*
-- (an API key, a feed secret) reachable by whoever holds the dashboard
-- password. Vault balances are already public on-chain — anyone can read
-- the same numbers off the vault account — so exposing them to a
-- read-only role discloses nothing the chain does not.
--
-- Be precise about the scope of that argument, because it is narrower than
-- the table: it covers the *inventory and book* columns. A few others
-- (`launch_tvl_usd`, `reference`, `skew_bps`, `basis`) are strategy
-- internals with no on-chain counterpart. They are not secrets, and a
-- dashboard role is the intended reader, so they stay — but "already public
-- on-chain" is not the reason they are safe.
--
-- No key, seed, or signer material appears in any column here, and none may
-- be added. Two columns need that stated as a mechanism rather than a
-- promise, because they carry arbitrary text a *venue* produced:
-- `tick_error` here and `feed_health.last_error` below. A transport error's
-- message routinely embeds the request URL, and a keyed endpoint carries its
-- credential in the query string — so an error message is an exfiltration
-- path for exactly the secret this comment disclaims, via a table the
-- dashboard role can read. The two are guarded by *different* mechanisms,
-- and the split is what a new writer of either column has to know:
--
--   * `tick_error` goes through the framework's `sanitize_error`, which
--     strips a URL query string wholesale. Its text comes from the Solana
--     RPC client, which has no redaction of its own, so the blunt strip is
--     the only guard available on that path.
--   * `feed_health.last_error` relies on the `feeds` transport instead.
--     `HttpClient::redact_query` replaces a credential's value by
--     registered parameter *name*, before the error is ever wrapped. That
--     is better than the blunt strip, not weaker: the benign parameters a
--     failed paged backfill is diagnosed from (which symbol, which
--     interval, which window) survive. It is why the health path
--     deliberately does not blanket-strip on top.
--
-- Be exact about the residual risk, because that mechanism is a deny-list.
-- It covers the parameter names an adapter registered on its client, via
-- `with_secret_query_param` or `with_secret_header`. A keyed adapter that
-- instead hand-passes its credential through a per-request query, under a
-- name registered nowhere, is covered by nothing and its error text would
-- land in this column in clear. Two things hold that shut today, neither of
-- them this comment: every source that currently writes here is keyless,
-- and `HttpClient`'s own docs make the registration discipline explicit at
-- the constructor. A schema comment cannot enforce it — which is the
-- reason the redaction lives on the client rather than at a call site.

-- One sample per market per tick.
--
-- Nullability is meaningful throughout and not incidental. A tick can end
-- at any of six points — the vault read failing, a frozen vault, a paused
-- composition, a halt, a reshape, an ordinary quote — and each knows
-- strictly less than the one after it. So a column is NOT NULL only if
-- *every* one of those paths can fill it honestly:
--
--   * `anchor` / `regime` / `health` / the two breach flags come from the
--     composed reference, which is computed before the tick is entered and
--     is therefore always known.
--   * `action` is always known because the paths that end before the
--     kill-switch policy runs have their own names for what they did (see
--     the values listed below).
--   * `fair` is NULL exactly when the composition paused; `basis` exists
--     only in an FX-anchored regime; `halt_reason` only under a halt.
--   * `skew_bps`, the inventory columns, and `frozen` / `reference_valid`
--     are NULL on the paths that return before valuing the vault — a
--     failed vault read knows none of them, and a frozen vault or a paused
--     tick never computes a skew.
--
-- A panel that plots NULL as zero would therefore lie — a zero skew and an
-- unknown skew are different facts, and so are an empty vault and an
-- unread one. The dashboards leave gaps instead.
--
-- `action` holds one of: `Quote`, `Reshape`, `FreezeSide`, `Halt` (the
-- kill-switch decision), plus `Pause` (no usable reference), `Frozen` (the
-- vault is frozen on-chain, so the bot idles), `TickError` (the tick failed
-- before deciding anything — `tick_error` carries what with), and
-- `Unknown`. The last four are not `Action` variants in the model; they are
-- the states a tick can be in that the policy never got to decide, and a
-- state timeline that omitted them would show gaps where the interesting
-- failures are.
--
-- `Unknown` is the one value no path currently writes, and it is enumerated
-- deliberately rather than by accident: it means a tick reached no decision
-- and did not fail either, which is a shape that should not exist. It is
-- kept mapped so that a future path added without an outcome shows up as
-- itself instead of borrowing the rendering of a real trading state. A
-- reader must treat `Unknown` as a defect signal, not as a quiet tick.
--
-- Note that `TickError` and `Halt` are not exclusive in the way a single
-- column suggests: a tick that decided `Halt` and then failed to send the
-- instruction records `action = 'Halt'` with a non-NULL `tick_error`. The
-- decision wins the column on purpose — it is the more alarming fact, and
-- the kill-switch alert keys on it — so any query counting tick failures
-- must test `tick_error IS NOT NULL`, never `action = 'TickError'`.
--
-- The enum-ish columns (`anchor`, `regime`, `health`, `action`,
-- `halt_reason`, `profile_kind`) are TEXT holding the Rust variant's
-- `Debug` name rather than a Postgres enum or a smallint code. A CHECK
-- constraint or enum type would have to be migrated in lockstep with every
-- variant added to the model, and the model is still moving; TEXT keeps a
-- new variant from turning a telemetry write into a constraint violation
-- that fails the write it is trying to report on.
CREATE TABLE maker_telemetry (
    ts               BIGINT           NOT NULL,
    -- The market's **symbol** (`EURC`), which is what a dashboard legend
    -- reads and what the roster keys on.
    market           TEXT             NOT NULL,
    -- The market account, so this table can be joined to the indexer's
    -- `fill_events` / `takes`, which key on the pubkey and know nothing of
    -- symbols. Without it the fills overlay has no join path at all.
    market_pubkey    TEXT             NOT NULL,
    -- The two mints' decimals. Static per market, so carrying them on every
    -- tick row is deliberate denormalization with one job: the indexer's
    -- `takes.avg_price` is a raw **atoms ratio**
    -- (`total_fill_quote / total_fill_base`) with no decimal scaling, so
    -- plotting it against `fair` is only correct once multiplied by
    -- `10^(base_decimals - quote_decimals)`. Today's roster is 6-vs-6
    -- throughout, which makes the unscaled ratio look right and hides the
    -- bug — exactly why the factor is published here rather than left for
    -- whoever writes the next panel to rediscover.
    base_decimals    SMALLINT         NOT NULL,
    quote_decimals   SMALLINT         NOT NULL,
    -- The composed mid (USDC per token), NULL when paused.
    fair             DOUBLE PRECISION,
    -- Three references that are routinely different, and the differences are
    -- the point:
    --   * `reference` — what this tick computed (skewed mid). A candidate.
    --   * `last_set_price` — what this process last actually stamped.
    --   * `on_chain_reference` — what the vault carries right now, which may
    --     predate this process entirely (a restart inherits it).
    -- The gap between `reference` and `on_chain_reference` is the drift the
    -- trigger policy is tolerating; a panel plotting only one of them cannot
    -- show it.
    reference        DOUBLE PRECISION,
    last_set_price   DOUBLE PRECISION,
    on_chain_reference DOUBLE PRECISION,
    -- Best bid / ask implied by the armed ladder's tightest level against
    -- `reference`. NULL for a side that is dark (a freeze-side reshape, a
    -- halt, or a book killed for staleness) — that is the whole point of
    -- recording them rather than deriving them in the dashboard, which
    -- could not tell a dark side from a missing sample.
    best_bid         DOUBLE PRECISION,
    best_ask         DOUBLE PRECISION,
    skew_bps         DOUBLE PRECISION,
    anchor           TEXT             NOT NULL,
    regime           TEXT             NOT NULL,
    health           TEXT             NOT NULL,
    degraded         BOOLEAN          NOT NULL,
    uncertain        BOOLEAN          NOT NULL,
    basis            DOUBLE PRECISION,
    basis_breach     BOOLEAN          NOT NULL,
    usdc_breach      BOOLEAN          NOT NULL,
    action           TEXT             NOT NULL,
    halt_reason      TEXT,
    profile_kind     TEXT             NOT NULL,
    -- Inventory by leg, valued in USD — which is the form both §2's skew and
    -- §4's floor reason about, so it is what an operator needs to see beside
    -- the decision those policies made.
    --
    -- The raw u64 atom balances are deliberately *not* mirrored here. They
    -- would need NUMERIC columns and a decimal crate to bind without i64
    -- truncation, and they are already recorded by the indexer on every
    -- `fill_events` row and readable from the vault account itself — so the
    -- cost would buy a third copy of a number two writers already publish.
    base_value_usd   DOUBLE PRECISION,
    quote_value_usd  DOUBLE PRECISION,
    tvl_usd          DOUBLE PRECISION,
    launch_tvl_usd   DOUBLE PRECISION,
    frozen           BOOLEAN,
    reference_valid  BOOLEAN,
    -- What the tick failed with, whatever `action` says. Rendered with its
    -- cause chain and truncated by the writer. This — not
    -- `action = 'TickError'` — is the test for "did this tick fail": a tick
    -- that decided and *then* failed keeps its decision in `action`, so the
    -- two do not partition. Covers propagated errors only; a failure the
    -- tick deliberately swallows (the quote-state write) does not appear.
    tick_error       TEXT,
    PRIMARY KEY (market, ts)
);

-- The dashboards and the heartbeat alert both scan recent rows across all
-- markets, which the `(market, ts)` primary key cannot serve — it orders by
-- market first, so "the last 15 minutes over every market" is a full scan.
CREATE INDEX maker_telemetry_ts_idx ON maker_telemetry (ts);

-- One row per market per leg per tick.
--
-- `leg` is the leg's role in the §1 composition (`fx`, `crypto_usdc`,
-- `usdc_usd`), never a venue.
--
-- **There is deliberately no "which feed supplied this" column, and that
-- is a consequence of the resolver rather than an omission.** A leg is a
-- *candidate set* resolved by consensus — several sources contribute and
-- the value is a summary of them (a median, or a designated source that
-- survived contradiction), so there is no single answering venue to name.
-- Recording one anyway would mean picking arbitrarily and presenting the
-- pick as authoritative, which is the same failure this schema already
-- refused when it split liveness away from readings. The three diagnostic
-- columns below say what is actually knowable: how well corroborated the
-- leg was, by how many sources, and who the suspect is when they
-- disagreed.
--
-- Per-source attribution returns as an **additive** migration once the
-- resolver exposes a contributor set with weights; that shape is already
-- decided, and it is not this table's business to approximate it early.
--
-- `age_secs` is the age the engine aged this reading by, carried instead of
-- being derived from `ts` because they are not the same quantity: the FX
-- anchor is aged from the *publisher's* clock (Pyth's `publish_time`), so a
-- reading received this tick can legitimately be minutes old, and that gap
-- is exactly what the staleness panel needs to show. A converted reading
-- (Kraken's token/USD divided by the peg) carries the *older* of its two
-- inputs' ages, so this is a claim about the whole quotient.
CREATE TABLE maker_legs (
    ts          BIGINT           NOT NULL,
    market      TEXT             NOT NULL,
    leg         TEXT             NOT NULL,
    -- The resolved value: the consensus summary of the candidate set, not
    -- any one source's print.
    value       DOUBLE PRECISION NOT NULL,
    age_secs    DOUBLE PRECISION NOT NULL,
    -- The symmetric confidence half-width, in `value`'s units, when the
    -- resolved reading carries one (Pyth Hermes publishes one; a plain REST
    -- quote does not). NULL means "no confidence notion" — never "certain".
    confidence  DOUBLE PRECISION,
    -- Whether the engine considered this reading usable this tick. A leg
    -- can be present and unfresh, which is why this is recorded rather
    -- than inferred from a staleness bound the dashboard would have to
    -- know.
    fresh       BOOLEAN          NOT NULL,
    -- How well corroborated the leg was, as the Rust variant's `Debug`
    -- name — the same convention every enum-ish column in `maker_telemetry`
    -- uses, because an alert matches these literally and a second
    -- convention in one row is how a row that should fire an alert drifts
    -- from what that alert keys on.
    --
    -- **Six values, and a reader must enumerate all six**: `Absent`,
    -- `Corroborated` (3+ sources inside the band), `Agreed` (exactly two),
    -- `SingleTrusted` (one source, designated believable alone),
    -- `SingleUnverified` (one source, nothing corroborating it), and
    -- `Dispersed`.
    --
    -- The last two must never be collapsed. `SingleUnverified` is the
    -- *steady state* for a market with no second source — most of this
    -- roster — not a fault, and it is the only signal that a market is
    -- being quoted off one unchecked feed. Merging it into `SingleTrusted`
    -- would erase exactly that, and worst on the thin markets where it
    -- matters most.
    consensus_state    TEXT      NOT NULL,
    -- How many healthy sources contributed to the resolution. INTEGER, and
    -- the resolver's count is a `usize`, so the writer saturates rather
    -- than wrapping.
    contributor_count  INTEGER   NOT NULL,
    -- The source **furthest from** the consensus when the set was
    -- dispersed: the suspect, the *least* representative member.
    --
    -- Emphatically **NOT** "the feed that answered" — that reading is
    -- exactly backwards, since this names the source to distrust. The
    -- column is named for the condition that populates it precisely so the
    -- attribution misreading is unavailable. NULL whenever the leg was not
    -- dispersed, which is the normal case.
    --
    -- The value is a `feeds` source name, so it joins to `feed_health` —
    -- but note the spot source is named per product (`coinbase:EURC-USDC`)
    -- while the resolver offers the bare venue (`coinbase`), so that one
    -- join is a prefix match on the `:`, not equality.
    dispersion_outlier TEXT,
    PRIMARY KEY (market, leg, ts)
);

CREATE INDEX maker_legs_ts_idx ON maker_legs (ts);

-- Current status of every registered feed source, upserted in place.
--
-- Written by the framework's own metrics seam, so a source that is merely
-- *registered* gets a row with no per-feed wiring — a later venue adapter
-- (Orca / Raydium / Aerodrome) appears here the first time it polls.
--
-- Deliberately last-state rather than a history. The question this answers
-- is "is this poller alive right now", which the TUI's feed-health pane and
-- the staleness alert both ask of the latest state only; keeping a row per
-- poll would grow without bound at the poll cadences (5 s for Hermes) to
-- serve a question nothing asks.
--
-- `last_ok_at` and `last_error_at` are tracked separately, and neither is
-- "the last time we heard from this feed": a source that is failing
-- updates `last_error_at` on every retry, so a single `updated_at` would
-- keep looking fresh while the feed was dead. Staleness is measured off
-- `last_ok_at` alone.
CREATE TABLE feed_health (
    feed            TEXT    PRIMARY KEY,
    -- 'ok' or 'error' — the outcome of the most recent poll.
    status          TEXT    NOT NULL,
    last_ok_at      BIGINT,
    last_error_at   BIGINT,
    -- The most recent error text, retained after recovery so an operator
    -- can see what a now-healthy feed was failing with. Cleared by
    -- nothing; read together with `last_error_at`.
    last_error      TEXT,
    -- Records in the most recent successful batch, and whether the source
    -- reported it had reached the present. A poll source stuck mid-backlog
    -- reports `caught_up = false` for consecutive turns, which is the only
    -- lag signal the framework can offer generically.
    last_records    INTEGER,
    caught_up       BOOLEAN,
    -- Turn counters, so a flapping feed is distinguishable from a
    -- cleanly-running one at a glance. Cumulative since the *row* was
    -- created, not since the process started: the row outlives a bot
    -- restart, so these keep accumulating across restarts and a ratio
    -- between them is only meaningful over a window (take a delta), never
    -- as an absolute.
    ok_count        BIGINT  NOT NULL DEFAULT 0,
    error_count     BIGINT  NOT NULL DEFAULT 0,
    -- The last time *either* outcome was recorded. Deliberately not what
    -- staleness reads — that is `last_ok_at`. This one answers "is anything
    -- still driving this source at all", which a feed erroring every retry
    -- also satisfies.
    updated_at      BIGINT  NOT NULL
);
