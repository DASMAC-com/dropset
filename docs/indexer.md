<!-- cspell:word Defi -->

<!-- cspell:word Fargate -->

<!-- cspell:word memcmp -->

<!-- cspell:word ohlcv -->

<!-- cspell:word PostgREST -->

<!-- cspell:word upserts -->

# Dropset Indexer — Prototype Design

The off-chain **event indexer**: it subscribes to the program's
`emit_cpi!` events on a cluster, decodes them, persists the raw legs
plus a few derived rollups to a store, and serves the `/v1` REST
surface that `interface.md` promises. This document is the design
**spec** for the prototype — what to build, what to reuse, and the
decisions taken before any code lands.

**Doc boundary.** This is a *consumer* of the contract in
[`interface.md`](interface.md) §1–§2 (the event schema and wire
format) and §5 (`/v1`). It never defines events — the program's
`#[event]` structs, surfaced in the IDL, are the source of truth.
Dependency flows `interface.md → indexer.md`; `interface.md` never
references this file.

**Status.** Greenfield — there is no `indexer/` tree yet. The decode
half is already solved in the repo (see §2); this spec is the plan for
the ingest, store, aggregate, and serve halves around it.

______________________________________________________________________

## 1. Goals and non-goals

**Goal — a faithful, restartable prototype.** Subscribe to Dropset
events on localnet, decode every leg, persist them under the frozen
primary key, derive take-level and book rollups, and serve `/v1`. Ship
it as a Docker service the localnet stack brings up with one `make`
target, on a path that extends cleanly to an AWS deploy.

**In scope (prototype):**

- Ingest + decode of all emitted events (`FillEvent`, `Deposit`,
  `Withdraw`, `CreateVault`, `CloseVault`, `FreezeVault`, `Realize`,
  and the admin retuning events).
- A Postgres store keyed on
  `(slot, txn_index, signature, event_ordinal)`.
- A watermarked aggregator that groups per-leg fills into takes and
  maintains a per-market rollup (volume and last price). TVL /
  vault-inventory and OHLCV candles are deferred (§5, §9).
- A hand-written `/v1` REST service over what the store holds: fills,
  takes, per-market stats, and the raw event log. The richer
  vendor-shaped surfaces (vaults, positions, book/depth) are deferred.
- A localnet Docker service + `make` target; an AWS-shaped deploy
  path.

**Out of scope (prototype):** the aggregator-vendor adapters — only
some of which are transforms over `/v1` at all; §11 scopes each of the
seven, authored later against vendor fixtures — the wash-clustering
pipeline (off-chain, separable), and the realtime push channel
(WebSocket / SSE — note the seam, defer the implementation).

______________________________________________________________________

## 2. What already exists — the reuse surface

The prototype is not built from zero. Three pieces already carry most
of the decode and the schema-of-record:

- **Event extraction + decode.** The shared decoder now lives in
  `dropset_sdk::events` (built in this work): `EVENT_IX_TAG_LE`, the
  IDL-pinned event discriminators, `strip_event_tag`,
  `decode_event_payload`, and a `DropsetEvent` enum. It walks a
  transaction's inner instructions, strips the `[tag][discriminator]`
  envelope, and decodes each body via borsh against the generated
  struct — the SDK's `FillEvent` carries explicit `pad` / `pad2`
  fields, so one borsh read is byte-identical to the on-chain
  `bytemuck` `repr(C)` layout. This is the reference implementation of
  `interface.md` §2's "walk inner instructions, strip the envelope"
  extraction; the indexer's `decode.rs` adapts it to live transaction
  meta. The pre-existing decoder in
  `programs/dropset/tests/common/events.rs` is the same algorithm against
  litesvm's `TransactionMetadata`; it should adopt the SDK module so the
  two never drift (a noted follow-up — the test harness keeps its
  field-level assertions either way).
- **Post-extraction codecs.** The Codama-generated event structs in
  [`sdk/rs/src/generated/types/`](../sdk/rs/src/generated/types/)
  (`fill_event.rs`, `deposit_event.rs`, …) are the typed decode
  targets — exactly the "Codama supplies only the post-extraction
  codec" split the contract describes. The indexer depends on the
  generated SDK crate rather than re-deriving any layout.
- **Localnet Docker stack.**
  [`infra/localnet/`](../infra/localnet/) already orchestrates a
  seeded validator + explorer. The indexer is a new service in that
  stack, not a new stack.

The current **maker-bot** decodes fills the lighter way — a blocking
`logsSubscribe` → `getTransaction` → inner-instruction walk — to drive
its own position; it is a working precedent for the live walk, but the
indexer needs durable, ordered, replay-safe ingest rather than a
bot's best-effort tail.

### Lesson from an earlier prototype

An earlier in-house streaming prototype proved the geyser path (a
`yellowstone-grpc-client` subscription filtered on the
`event_authority` PDA + a market-account memcmp) and a `cargo-chef`
multi-stage Docker build — both liftable in shape. Its event envelope
was an **older batched** model (a single flush instruction carrying a
tagged event stream), not today's per-leg `emit_cpi!`, so the *decode*
is replaced by §2's current extractor, but the *subscription setup*
transfers. Its one instructive mistake: the stream was **print-only —
no persistence**. The events flowed to stdout and were discarded,
which is why a store was never bolted on cleanly. This prototype writes
to the store from the first commit.

______________________________________________________________________

## 3. Pipeline

```text
 cluster ──▶ ingest ──▶ decode ──▶ store(raw) ──▶ aggregate ──▶ /v1
 (RPC poll;  (filter     (events.rs   (Postgres:    (watermarked   (Axum
  geyser      event-      walk +       fill_events   worker:        REST)
  next)       authority)  SDK codec)   + JSONB, PK)  legs→takes,
                                                     market stats)
```

Each stage is idempotent on the primary key, so a replay (restart,
backfill, or a re-delivered slot) never double-counts.

______________________________________________________________________

## 4. Decisions

The transport / persistence choices were deferred to a research pass
over established event-indexer designs and `interface.md`. The findings
point to one coherent stack.

### 4.1 Language — Rust

The decode reference, the SDK codecs, and `math-core` are all Rust, as
are the mature order-book event indexers this design drew on. A Rust
indexer reuses the in-repo extractor and codecs directly and shares the
consensus math instead of re-deriving NAV / PnL. No second-language
port for the prototype.

### 4.2 Transport — geyser, with an RPC-poll dev fallback

`interface.md` §2 already prescribes it: *"the geyser transaction
subscription filters on Dropset's `__event_authority` PDA."* Geyser
(Yellowstone gRPC) delivers inner instructions directly, is the
production-faithful path, and is the one the earlier prototype proved.
The prototype targets geyser.

Because `emit_cpi!` events ride **inner instructions, not logs**, the
lighter `logsSubscribe` path the maker-bot uses needs a follow-up
`getTransaction` to see the inner instructions anyway — so a
**`getSignaturesForAddress` + `getTransaction` poll at `finalized`**
is the natural low-dependency *dev fallback* (no geyser plugin needed
for a first bring-up), behind the same decode + store seam. This
poll-at-`finalized` shape is a proven production pattern for Solana
order-book indexers, not a toy.

**Built first: the RPC poll.** The prototype's `ingest.rs` implements
this poll (`RpcPollSource`), keeping the dependency tree to the same
solana 3.x line as the maker-bot and needing no validator plugin.
Geyser is the documented next step behind the same `poll` shape — it
supplies inner instructions directly and the true per-block
transaction index (the RPC path leaves `txn_index` at `0`, which is
safe because the globally-unique signature already keys the row).

### 4.3 Persistence — Postgres

Every reference indexer persists to Postgres; the aggregation idioms
the contract needs (incremental upserts, optional materialized views,
a PostgREST shortcut) all assume it, and an AWS deploy maps it to RDS.
SQLite would force a rewrite at the AWS boundary and can't host the
rollup idioms. Use **`sqlx`** (compile-checked queries, async, light
migration runner) over diesel for the prototype's smaller surface.

### 4.4 API — hand-written `/v1` (Axum)

`interface.md` §5 is explicit: one REST service, `/v1` only, OpenAPI
3.1 over the owned surface. That is a hand-written Axum service (a
small Axum + RPC-client service, a shape the repo already uses
elsewhere), **not** PostgREST. PostgREST stays in mind only as a throwaway
read-only shortcut while `/v1` is still being shaped; the committed
surface is the typed `/v1`.

______________________________________________________________________

## 5. Storage schema

Two tiers, the standard pattern for event indexers: **raw, immutable,
append-only** event tables, and **derived** rollup tables the
aggregator owns. Never aggregate at query time.

### Tier 1 — raw events

The hot, high-cardinality **`fill_events`** is a typed table — one
column per `FillEvent` field. Every other event — the lifecycle events
(`Deposit`, `Withdraw`, `CreateVault`, `CloseVault`, `FreezeVault`,
`Realize`) and the admin retuning events — lands in a single generic
**`events`** table at full fidelity: the key columns, a `kind`, the
`market`, and the decoded JSON `payload`. Both tables carry the same
primary key, the one frozen in `interface.md` §1:

```text
PRIMARY KEY (slot, txn_index, signature, event_ordinal)
```

`event_ordinal` is the inner-instruction index (heap-pop order). Every
write is an idempotent `INSERT … ON CONFLICT DO NOTHING`, so a replayed
slot is a no-op — the PK *is* the dedup contract, end to end. Promoting
the cold events out of the JSONB `events` table into their own typed
tables is the natural next step; the generic table keeps full fidelity
meanwhile.

### Tier 2 — derived rollups

Owned by the aggregator (§6), each carries its own watermark:

- **`takes`** — one row per `(signature, txn_index)` group of fill
  legs: `total_fill_base`, `total_fill_quote`, `total_taker_fee`,
  `avg_price` (= `total_fill_quote / total_fill_base`), `market`,
  `taker`, `side`. This is the take-level view the contract says is
  *derived, not emitted*.
- **`market_stats`** — per-market last price and raw volume. The
  self-trade-adjusted volume columns are reserved (nullable) pending
  the off-chain wash-clustering pipeline — never silently netted, per
  §1 "volume integrity". A USD `reserve_in_usd` waits on a price feed
  (open: h) and is likewise deferred.
- **`vault_inventory`** *(deferred — not built in the prototype, §9)* —
  per vault, the latest
  `(base_atoms_after, quote_atoms_after, nonce_after)`, for TVL and a
  book/depth endpoint. Live book depth would be reconstructed from
  `LiquidityProfile` account state, not from events (the hot path
  emits nothing).

A candle / OHLCV table is the obvious next rollup (the idempotent
`ON CONFLICT … DO UPDATE` candlestick fold is the template:
`open=COALESCE(first)`, `high=GREATEST`, `low=LEAST`,
`close=EXCLUDED`, `volume=volume+EXCLUDED`, ordered by the PK) but is
deferred past the first prototype.

______________________________________________________________________

## 6. Aggregation

A **watermarked worker**, not database triggers — the standard shape
for event-indexer aggregation. The worker reads the fill legs past a
persisted cursor (the singleton `indexer_cursor`, holding the last
folded `(slot, txn_index, event_ordinal, signature)`), then for each
touched `(signature, txn_index)` re-reads that take's **full** leg set
and recomputes its row — so the upsert is a full recompute and
re-running converges on the same value, idempotent without a per-leg
ledger. Wrapping the pass in a `repeatable read` transaction and adding
an `aggregated_events` ledger are later hardening; the prototype relies
on the cursor plus idempotent recompute.

The **per-leg → take** grouping is the load-bearing case: group the
raw `fill_events` of one transaction by `(signature, txn_index)`, sum
`fill_base` / `fill_quote` / `taker_fee_atoms`, derive `avg_price`,
and upsert one `takes` row. Triggers are reserved for a future
`pg_notify` realtime fan-out, never for the rollup math.

______________________________________________________________________

## 7. Reorg and finality

The PK absorbs replays, so the only real question is non-final slots.
For the prototype:

- Subscribe / poll at **`finalized`** for the canonical store — no
  reorg below finality to handle. Localnet finalizes effectively
  instantly, so this is free in dev.
- A lower-latency `confirmed`/`processed` tap (for a future realtime
  channel) is an explicit later seam: it would write to the same tables
  and let the `(slot, …)` PK + `ON CONFLICT` reconcile when the slot
  finalizes. Not built in the prototype.

______________________________________________________________________

## 8. Layout and deploy

### Crate / module layout

A single `indexer/` crate for the prototype, two binaries, modules per
stage — split into a workspace only if a stage grows independently:

```text
indexer/
  Cargo.toml
  migrations/      # sqlx SQL migrations (0001_init.sql)
  queries/         # externalized .sql, loaded via include_str!
  src/
    config.rs      # env-driven config (db / rpc / program id)
    model.rs       # row types, /v1 wire shape, event → JSON / columns
    decode.rs      # dropset_sdk::events walk, on live tx meta
    ingest.rs      # RpcPollSource (geyser is the next step)
    store.rs       # sqlx pool, migrations, ON CONFLICT writers + reads
    aggregate.rs   # watermarked legs→takes + market rollups
    api.rs         # Axum /v1 router
    bin/indexer.rs # ingest + decode + store + aggregate worker
    bin/api.rs     # the /v1 service
```

The wire-format extractor is **shared, not forked**: it lives in
`dropset_sdk::events`, and `decode.rs` is a thin adapter that assigns
each decoded event its coordinates. The program test harness in
`programs/dropset/tests/common/events.rs` is the same algorithm and
should adopt the SDK module (noted in §9) so the test's "double as a
wire-format pin" property keeps covering the indexer's decode too.

### Localnet

A new service in [`infra/localnet/`](../infra/localnet/): a Postgres
container + the indexer + the `/v1` API, wired to the seeded validator.
A `make indexer-up` target brings the stack up; a `cargo-chef`
multi-stage Dockerfile keeps the image lean. The maker-bot seeding the
market gives the indexer a live event source on localnet.

### AWS

The prototype's compose maps cleanly to the deploy target: the
indexer + API as ECS/Fargate tasks (or a single task for the
prototype) against an RDS Postgres. No IaC in the prototype beyond a
note of this path; choosing geyser + Postgres now is what keeps the
AWS step a deploy rather than a rewrite.

______________________________________________________________________

## 9. Open questions

- **(f) wire encoding.** Price / amount representation (string vs
  scaled integer vs decimal) is still open in `interface.md` §7. The
  store should hold the raw atoms / `Price` as decoded and defer the
  display encoding to `/v1` serialization, so the column types don't
  have to be revisited when (f) lands.
- **(h) price feed.** `reserve_in_usd` and any USD volume need the
  conditional USD/FX feed. The prototype leaves the USD columns
  nullable and populates the native-atom figures only.
- **Shared-extractor home — resolved: `dropset_sdk::events`.** The
  remaining follow-up is migrating
  `programs/dropset/tests/common/events.rs` onto the SDK module so the
  two cannot drift (the harness keeps its field-level assertions).
- **SDK `serde` feature is non-compiling** (pre-existing): the Codama
  instruction-args structs derive serde on a bare `Pubkey` and a
  `[u8; 160]`, neither serde-supported as generated. The indexer
  sidesteps it (decode is borsh; `/v1` JSON is built in `model.rs`),
  but the feature wants a Codama-visitor fix — a separate follow-up.
- **RPC-poll backlog window.** The poll fetches at most
  `signature_batch_limit` newest-first signatures per tick and then
  advances the cursor to the newest — so a backlog larger than one
  batch (a long gap, or the first poll after downtime) skips the
  middle. Fine for steady localnet flow; the fix (page with `before`
  until the window drains, or don't advance on a saturated batch) lands
  with the geyser path.
- **Realtime channel.** WebSocket vs SSE vs PostgREST + `pg_notify`
  for the eventual push surface — out of scope for the prototype, seam
  noted in §6 / §7.

______________________________________________________________________

## 10. Prior art

The design follows patterns established by mature on-chain order-book
event indexers; how far this prototype takes each:

- A **processor → Postgres ← watermarked aggregator** topology, with a
  thin REST layer (hand-written `/v1` here; PostgREST is a known
  shortcut) over the store. *Adopted.*
- A **raw tier keyed on a uniform event coordinate** with idempotent
  `ON CONFLICT` writes, and **derived rollups** folded by a watermarked
  worker, never at query time. *Adopted* — though the raw tier is one
  typed `fill_events` table plus a generic JSONB `events` table (full
  table-per-event, and an `aggregated_events` ledger, are the next
  step, not yet built; idempotent recompute stands in, §6).
- The **candlestick fold** (`open=COALESCE(first)`, `high=GREATEST`,
  `low=LEAST`, `close=EXCLUDED`, `volume=volume+EXCLUDED`) as the
  template for the deferred OHLCV rollup. *Deferred.*
- **Poll-at-`finalized`** ingest as the low-dependency path, with the
  event coordinate PK absorbing replays. *Adopted.*

The earlier in-house streaming prototype (see §2) is the one local
precedent the subscription + Docker shape transfers from; its
print-only, no-persistence design is the mistake this prototype
avoids.

______________________________________________________________________

## 11. Aggregator-vendor adapter scoping

§1 defers the aggregator-vendor namespaces as "thin transforms over
`/v1`." Checked against each vendor's current (2025–2026) onboarding,
that framing holds cleanly for essentially **one** path (CoinGecko),
partly for a **second** (DeFiLlama's volume adapter), and **not at all**
for the other five on [`interface.md`](interface.md) §4's consumer
list — they either never touch `/v1` or need surfaces it does not yet
expose. This section records where each lands, so the deferral is a
decision rather than a blank. It is a *scoping* pass — no adapter is
built here; the origin is `interface.md` §4's table, refined per vendor.

The seven vendors fall into **three integration shapes** (DeFiLlama
spans two — its volume and TVL adapters differ), and only the first is
the indexer's to serve:

| Vendor                        | Integration shape                                | Feeds from               | Adapter owner   | Needs from us                                                     |
| ----------------------------- | ------------------------------------------------ | ------------------------ | --------------- | ----------------------------------------------------------------- |
| **CoinGecko / GeckoTerminal** | Project-hosted REST adapter; the vendor polls it | **`/v1`**                | Ours            | GeckoTerminal on-chain DEX endpoints + new `/v1` surfaces (§11.5) |
| **DeFiLlama** (volume / fees) | `dimension-adapters` JS PR; may call `/v1`       | **`/v1`**                | Ours (PR)       | de-washed `dailyVolume` + `dailyFees`                             |
| **DeFiLlama** (TVL)           | `DefiLlama-Adapters` JS PR                       | **on-chain vault reads** | Ours (PR)       | vault balances (the deferred TVL rollup, §5)                      |
| **DEXScreener**               | Vendor-side crawler                              | on-chain (their parser)  | **Vendor**      | listing packet: program id + audit + decoder                      |
| **Birdeye**                   | Vendor-side crawler / BD partner                 | on-chain (their parser)  | **Vendor**      | same listing packet                                               |
| **Jupiter**                   | Off-chain quoting adapter (`Amm` trait)          | **on-chain state**       | Ours (SDK)      | slippage-bounded take ix + account metas                          |
| **DFlow**                     | Routing venue (aggregator)                       | **on-chain state**       | Ours + DFlow BD | same as Jupiter                                                   |
| **Titan**                     | Meta-aggregator — inherits via Jupiter           | via Jupiter              | via Jupiter     | nothing direct                                                    |

### 11.1 Transforms over `/v1` — the indexer's actual surface

Only **CoinGecko / GeckoTerminal** and **DeFiLlama's volume adapter**
are genuine transforms over `/v1`, and both need `/v1` to grow first.

- **CoinGecko / GeckoTerminal.** Non-EVM Solana venues are not
  auto-crawled, so the listable path is a small **project-hosted,
  read-only REST adapter** conforming to GeckoTerminal's on-chain DEX
  spec — `/latest-block`, `/asset`, `/pair`, `/events` — which
  GeckoTerminal **polls** (public, no auth). Submission is a form plus a
  manual review (order of a week or two), free. The adapter itself is a
  thin transform, but `/v1` must first expose a **block-height +
  timestamp cursor**, token / pair metadata, per-fill **events shaped as
  swaps**, and **USD reserves / volume**. An order book has no pool
  reserves, so `reserves` / `priceNative` are **synthesized** — price
  from last-fill / mid, "liquidity" from matchable depth or vault TVL;
  the LP `join` / `exit` event types have no analog and are omitted (or
  mapped to vault deposit / withdraw). The USD figures wait on the
  conditional price feed (open: h, §9).
- **DeFiLlama (volume / fees).** A JavaScript PR to `dimension-adapters`
  whose per-day `fetch` may legitimately call `/v1`. It must report
  **self-trade-adjusted `dailyVolume`** (DeFiLlama de-washes) and
  `dailyFees`. So `/v1` owes a **date-parameterized, de-washed volume**
  figure — which waits on the wash-clustering pipeline (§1); the
  adjusted columns are already reserved nullable in `market_stats` (§5) —
  plus a fee total. Raw volume ships today; the de-washed flavor is the
  added surface.

### 11.2 Vendor-side crawlers — no `/v1` role (DEXScreener, Birdeye)

Both build their **own** on-chain parser and consume nothing from
`/v1`; their public APIs are read surfaces, not submission channels. Our
deliverable is a **listing packet, not code**: a stable program id, an
open-source + audited program, and an IDL / reference decoder — which
`dropset_sdk::events` (§2) already is — so the vendor can decode fills.
Onboarding is relationship-driven: DEXScreener via a Discord request
gated on liquidity / volume; Birdeye via its BD channel, likely a paid
partnership. Both model a venue as an AMM pool (liquidity = pooled
reserves), so expect the same order-book-mapping friction as §11.1's
CoinGecko case; the order-book precedent (OpenBook, Phoenix are indexed)
shows it is workable but bespoke on the vendor's side. Because these
bypass `/v1`, the indexer's only obligation here is to keep the shared
decoder (§2) authoritative and versioned.

### 11.3 Routers / order-flow — a different model (Jupiter, DFlow, Titan)

Not an indexer concern at all, and not a `/v1` consumer. A router
integration is an **off-chain quoting adapter** that reads
market-account state and prices a swap, backed by a **slippage-bounded
take CPI** into the program — the SDK + on-chain layer
(`interface.md` §4's "B1"), never the REST surface.

- **Jupiter.** Implement `jupiter-amm-interface`'s `Amm` trait
  (`from_keyed_account`, `get_accounts_to_update`, `update`, `quote`,
  `get_swap_and_account_metas`). Jupiter **forks** the adapter and calls
  `quote()` in-process against cached account bytes — network calls are
  forbidden inside it. The gate is code health, an independent security
  audit, demonstrated traction, and a reputable team; no fee. It leans
  entirely on the on-chain take ix exposing a real `Price` limit, a
  deterministic revert, and a stable `AccountMeta` set — exactly
  `interface.md` §4's "Routers (B1)" note.
- **DFlow.** An aggregator that treats venues as interchangeable
  liquidity sources (AMMs, prop AMMs, order-book venues alike); becoming
  a routable venue is the same off-chain-quote + on-chain-swap shape as
  Jupiter, plus DFlow's own BD onboarding. It reads chain state, not
  `/v1`.
- **Titan.** A **meta-aggregator** sitting above Jupiter / DFlow / its
  own router, so a Dropset market becomes routable in Titan **for free
  once it is in Jupiter**. Direct integration is a private BD
  conversation with no published interface — scope it as "via Jupiter,"
  not a separate build.

### 11.4 MCP servers — orthogonal to listing

Whether a vendor ships a Model Context Protocol server is **mostly
irrelevant to getting listed**. Official MCPs exist for CoinGecko
(read-only market data), Jupiter (docs-only), and DFlow (agent / trading
tools); DEXScreener, DeFiLlama, and Birdeye have only community wrappers
(and "Birdeye MCP" collides with an unrelated reputation-SaaS product —
not the crypto-data vendor); Titan has none. Every one is a
**consume-data / agent-tooling** surface, not a submit-your-DEX channel,
so none shortcuts the integration work above. DFlow's is the only one
with operational weight, and only as a consumer of DFlow's routing —
still not a listing path for us.

### 11.5 Net new `/v1` surfaces this implies

Only §11.1 drives indexer work, and every surface it needs is a
**pre-existing deferral** now tied to a concrete consumer:

- a **block-height + timestamp cursor** and an **events-as-swaps**
  projection (GeckoTerminal `/latest-block` + `/events`);
- **USD reserves / volume** — the deferred `vault_inventory` / TVL
  rollup (§5 tier 2) plus the conditional price feed (open: h, §9);
- **de-washed `dailyVolume` + `dailyFees`** — the wash-clustering
  pipeline (§1) feeding the already-reserved `market_stats` columns (§5).

None sit on the prototype's critical path; the crawler and router
vendors (§11.2, §11.3) add **no** `/v1` surface at all.
