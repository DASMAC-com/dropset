<!-- cspell:word Defi -->

<!-- cspell:word ohlcv -->

<!-- cspell:word rollups -->

# Dropset Interface Spec

The **consumer/client contract** for Dropset: the on-chain events the indexer
consumes, their wire representation, the order-book venue mapping, the
integrator surfaces, the REST API, and the SDK. It is the home for everything
an outside consumer or client touches.

**Doc boundary.** Dependency flows one way: **`interface.md` → `architecture.md`
→ IDL**. This document references *down* into the protocol spec
(`architecture.md`) and the generated IDL; `architecture.md` is self-contained
and never references this file. The canonical event schema is the program's
Anchor `#[event]` structs as surfaced in the IDL — the field lists below
**mirror** that schema for human readers.

**Status.** The core flows — `create_vault` / `deposit` / `withdraw` / `swap` —
are implemented and emit the events below; this spec is the consumer contract
for those events plus the not-yet-built integrator, REST, and SDK surfaces
(§4–§6). Items marked **(open: x)** depend on a decision tracked in
**§7, Open decisions affecting this contract**.

______________________________________________________________________

## 1. Event contract

Events are emitted on cold paths only; the hot path
(`SetReferencePrice`/`SetLiquidityProfile`) emits nothing (see
**architecture.md → Events and emission**). A single take can match many levels
across many vaults; **every leg is recorded — full fidelity, never dropped**,
which is why fills ride as `emit_cpi!` inner-instruction data rather than logs
(`sol_log_data` silently truncates past the ~10 KB-per-tx ceiling, which a
level-blasting sweep would hit). A single combined **`FillEvent`** is emitted
**per matched `(sector_idx, level_idx)` leg** — one `emit_cpi!` per leg, in
heap-pop (match) order. Each leg carries both the per-leg fill *and* the
take-level context (`market` / `taker` / `side` / `taker_fee_atoms`), so the
indexer reconstructs a take by grouping the legs of one transaction; there is
**no separate take-level event**.

### `FillEvent` — one per matched (sector_idx, level_idx) leg

Fixed-size, `#[event(bytemuck)]`. Fields (in wire order; the `_pad` / `_pad2`
gaps are bytemuck alignment padding and carry no data):

| field                                    | meaning                                                                                                         |
| ---------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `market`                                 | the market account                                                                                              |
| `taker`                                  | the transaction initiator                                                                                       |
| `leader`                                 | the vault's economic owner (carried directly — sectors are reused, so an index is not a stable attribution key) |
| `quote_authority`                        | the delegated quoting wallet (for off-chain wash clustering)                                                    |
| `side`                                   | taker side: `0` = ask-side (taker **Buy**), `1` = bid-side (taker **Sell**)                                     |
| `sector_idx`                             | the matched vault's sector index — reused across vaults, so attribute via `leader` / `quote_authority`          |
| `level_idx`                              | which `LiquidityProfile` level on that vault                                                                    |
| `fill_base`                              | base atoms moved on this leg                                                                                    |
| `fill_quote`                             | quote atoms moved on this leg                                                                                   |
| `fill_price`                             | the level's absolute `Price` this leg filled at                                                                 |
| `base_atoms_after` / `quote_atoms_after` | the vault's inventory after this leg                                                                            |
| `nonce_after`                            | `market.nonce` after this leg's per-leg bump (price-time priority / dedupe ordering)                            |
| `taker_fee_atoms`                        | protocol taker fee charged on this leg, retained in the output asset                                            |

### Take-level aggregation — derived, not emitted

There is no take-level event. The indexer recovers per-take figures by grouping
the `FillEvent` legs of one transaction (they share `market` / `taker` / `side`
and the transaction coordinates below):

- `total_fill_base` / `total_fill_quote` — sum `fill_base` / `fill_quote` across
  the take's legs;
- total taker fee — sum `taker_fee_atoms` across the legs;
- `avg_price` — derived (`total_fill_quote / total_fill_base`).

### Lifecycle / liquidity events

- **`Deposit` / `Withdraw`** — inventory deltas (join/exit liquidity; feed TVL
  and per-depositor cost-basis).
- **`CreateVault`** — a new vault/leader enters a market.
- **`CloseVault`** — a leader moves their vault from the active DLL to the
  tombstone DLL: matching stops, but depositor flows stay open until the vault
  drains.
- **`FreezeVault`** — an admin freezes a vault: it stays on the active DLL
  (existing levels still match until expiry) but can no longer be re-quoted.
- **`Realize`** — performance-fee accrual (leader economics).

### Dedupe / primary key

The event primary key is **`(slot, txn_index, signature, event_ordinal)`**.
`event_ordinal` is assigned in **heap-pop (match) order**, independent of flush
boundaries; `base_atoms_after` / `quote_atoms_after` snapshot the vault after
that leg. The emit model is **per-leg emit** — each matched leg is its own
`emit_cpi!` `FillEvent`, dispatched one at a time — so `event_ordinal` is the
inner-instruction index within the transaction, counting **all** Dropset
self-CPI inner instructions (stable across replay).

Plain `(slot, txn_index, signature)` is insufficient: it collides across the N
legs of one take.

### Volume integrity

Self-trade / wash is **detected off-chain** (wallet clustering over the
`taker` / `leader` / `quote_authority` exposed on every leg) — there is no
on-chain wash flag (a fresh wallet defeats a signer check; no leader allowlist).
Publish **both raw and self-trade-adjusted** volume; never silently net.

______________________________________________________________________

## 2. Wire representation

An `emit_cpi!` event is a **self-CPI inner instruction** (not a `Program data:`
log). Its instruction data is:

```text
[ EVENT_IX_TAG_LE  (8 bytes, 0x1d9acb512ea545e4) ]
[ event discriminator (8 bytes) ]
[ borsh-serialized event fields ]
```

emitted under **Dropset's own `__event_authority` PDA** (the program's event
authority — **not** any other program's).

**Indexer extraction.** The geyser transaction subscription **filters on
Dropset's `__event_authority` PDA**; an event inner instruction is recognized by
**program-id + the `EVENT_IX_TAG` prefix** (not a one-byte tag). The indexer
**walks inner instructions** to locate and strip the `[tag][discriminator]`
envelope; the Codama-generated struct codec then decodes the borsh body
(**Codama supplies only the post-extraction codec**, not the extraction).

**Why inner-instruction, not logs.** Full fidelity. Inner-instruction data is
not subject to the runtime's ~10 KB cumulative log-bytes-per-tx limit, so a
take that blasts through many levels **never drops a leg**; `sol_log_data`/
`emit!` would silently truncate. The cost is the two appended accounts
(`event_authority` + `program`) on every emitting ix — **trivial on the fill**
(the whole book is one market account) but a real budget item for **routers**
(§4). If a router's multi-hop account budget ever binds, the escape hatch is a
**bare self-CPI** (+1 account — drop the `event_authority` PDA); origin is
authenticated off-chain by program id + instruction binding, so the auth PDA is
optional.

**Price / amount encoding (open: f).** Choose one representation (string vs
scaled integer vs decimal) as the single source of truth shared by the
`#[event]` structs, the SDK, and the OpenAPI types; CI asserts `/v1` types match
SDK-generated types.

______________________________________________________________________

## 3. Venue mapping — order book (not AMM pool)

Dropset is an **order-book venue**, not a constant-product pool:

- **Price / depth** come from the **active, non-frozen, unexpired
  `LiquidityProfile` levels** across the market's vaults: best bid/ask + per-
  level depth. `spot` = mid (or last-fill VWAP); `q/b` is a *metric*, never the
  displayed price.
- **`reserve_in_usd` = the sum of matchable level sizes**, **not**
  `base_treasury.amount` / `quote_treasury.amount` (those custody pooled
  inventory across active **and tombstoned** vaults — total custody, not
  matchable liquidity; see **architecture.md → MarketHeader**). Its USD figure
  depends on the conditional price feed (§5, price feed).
- **Fee taxonomy:** only `taker_fee_rate` maps to a vendor "swap fee" field;
  the one-time `vault_open_fee` and the leader `perf_fee` are protocol-revenue /
  leader-economics, **not** per-trade fees.

______________________________________________________________________

## 4. Integrators / consumers

This is the **single home** for how every outside consumer integrates —
the aggregator vendors, the routers, and our own frontend / bots — split
by **how each ingests** and whether the integration is in our control.
`indexer.md` §11 owns only the *net-new `/v1` surfaces* the
transform-over-`/v1` vendors drive; the per-vendor scoping here is
authoritative.

Checked against each vendor's current (2025–2026) onboarding, §5's "thin
transforms over `/v1`" framing holds cleanly for essentially **one** path
(CoinGecko), partly for a **second** (DeFiLlama's volume adapter), and
**not at all** for the other five — they either never touch `/v1` or need
surfaces it does not yet expose. The seven aggregator vendors fall into
**three integration shapes** (DeFiLlama spans two — its volume and TVL
adapters differ), and only the first is the indexer's to serve.

| Consumer                              | Integration shape                                                                                                                                                                                    | Feeds from               | Control / adapter owner                               | Needs from us                                                              |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------ | ----------------------------------------------------- | -------------------------------------------------------------------------- |
| **CoinGecko / GeckoTerminal**         | Project-hosted, read-only REST adapter conforming to GeckoTerminal's non-EVM on-chain DEX spec (`/latest-block`, `/asset`, `/pair`, `/events`); the vendor polls it. Order-book shaped.              | **`/v1`**                | **Ours — fixtures-ready** (most listable Solana path) | GeckoTerminal on-chain DEX endpoints + new `/v1` surfaces (indexer.md §11) |
| **DeFiLlama** (volume / fees)         | `dimension-adapters` **JS PR** whose per-day `fetch` may call `/v1`. Report **self-trade-adjusted `dailyVolume`** (DeFiLlama de-washes) + `dailyFees`; OHLCV / order-book vendors get **raw** fills. | **`/v1`**                | Ours (PR + their review)                              | de-washed `dailyVolume` + `dailyFees`                                      |
| **DeFiLlama** (TVL)                   | `DefiLlama-Adapters` **JS PR**                                                                                                                                                                       | **on-chain vault reads** | Ours (PR)                                             | vault balances (the deferred TVL rollup)                                   |
| **DEXScreener**                       | Vendor-side crawler / partner request                                                                                                                                                                | on-chain (their parser)  | **Out of our control — "live on vendor"**             | listing packet: program id + audit + decoder                               |
| **Birdeye**                           | Vendor-side crawler / BD partner                                                                                                                                                                     | on-chain (their parser)  | **Out of our control — "live on vendor"**             | same listing packet                                                        |
| **Jupiter**                           | Off-chain **quoting Rust adapter (B1)** implementing the `Amm` trait                                                                                                                                 | **on-chain state**       | Ours (SDK) + Jupiter onboarding                       | slippage-bounded take ix + stable account metas                            |
| **DFlow**                             | Routing venue (aggregator); same off-chain-quote + on-chain-swap shape                                                                                                                               | **on-chain state**       | Ours (SDK) + DFlow BD                                 | same as Jupiter                                                            |
| **Titan**                             | Meta-aggregator — inherits via Jupiter                                                                                                                                                               | via Jupiter              | via Jupiter                                           | nothing direct                                                             |
| **beethoven (B2)**                    | CPI taker-swap composability. **Post-MVP, blocked on an upstream swap-context extension** (named owner + fallback). **Does not gate B1.**                                                            | on-chain CPI             | External dependency                                   | swap-context extension (named owner + fallback)                            |
| **Dropset frontend**                  | Consumes `/v1` (vaults, positions, prices).                                                                                                                                                          | **`/v1`**                | Ours                                                  | —                                                                          |
| **MM bots** (incl. the reference bot) | Read book / vault / fill state from `/v1`; drive quoting via the SDK.                                                                                                                                | **`/v1`** + SDK          | Ours / community                                      | —                                                                          |

### Transforms over `/v1` — the listable path (CoinGecko, DeFiLlama volume)

Only **CoinGecko / GeckoTerminal** and **DeFiLlama's volume adapter** are
genuine transforms over `/v1`, and both need `/v1` to grow first (the
net-new surfaces are tracked in **indexer.md §11**).

- **CoinGecko / GeckoTerminal.** Non-EVM Solana venues are not
  auto-crawled, so the listable path is a small **project-hosted,
  read-only REST adapter** conforming to GeckoTerminal's on-chain DEX
  spec — `/latest-block`, `/asset`, `/pair`, `/events` — which
  GeckoTerminal **polls** (public, no auth). Submission is a form plus a
  manual review (order of a week or two), free. The adapter is a thin
  transform, but an order book has no pool reserves, so `reserves` /
  `priceNative` are **synthesized** — price from last-fill / mid,
  "liquidity" from matchable depth or vault TVL; the LP `join` / `exit`
  event types have no analog and are omitted (or mapped to vault deposit
  / withdraw). The USD figures wait on the conditional price feed
  (open: h).
- **DeFiLlama (volume / fees).** A JavaScript PR to `dimension-adapters`
  whose per-day `fetch` may legitimately call `/v1`. It must report
  **self-trade-adjusted `dailyVolume`** (DeFiLlama de-washes) and
  `dailyFees`. Raw volume ships today; the de-washed flavor is the added
  surface.

### Vendor-side crawlers — no `/v1` role (DEXScreener, Birdeye)

Both build their **own** on-chain parser and consume nothing from `/v1`;
their public APIs are read surfaces, not submission channels. Our
deliverable is a **listing packet, not code**: a stable program id, an
open-source + audited program, and an IDL / reference decoder — which
`dropset_sdk::events` already is — so the vendor can decode fills.
Onboarding is relationship-driven: DEXScreener via a Discord request gated
on liquidity / volume; Birdeye via its BD channel, likely a paid
partnership. Both model a venue as an AMM pool (liquidity = pooled
reserves), so expect the same order-book-mapping friction as the CoinGecko
case; the order-book precedent (OpenBook, Phoenix are indexed) shows it is
workable but bespoke on the vendor's side.

### Routers / order-flow — a different model (Jupiter, DFlow, Titan)

A router integration is an **off-chain quoting adapter** that reads
market-account state and prices a swap, backed by a **slippage-bounded
take CPI** into the program — the SDK + on-chain layer (the "B1" note
below), never the REST surface.

- **Jupiter.** Implement `jupiter-amm-interface`'s `Amm` trait
  (`from_keyed_account`, `get_accounts_to_update`, `update`, `quote`,
  `get_swap_and_account_metas`). Jupiter **forks** the adapter and calls
  `quote()` in-process against cached account bytes — network calls are
  forbidden inside it. The gate is code health, an independent security
  audit, demonstrated traction, and a reputable team; no fee. It leans
  entirely on the on-chain take ix exposing a real `Price` limit, a
  deterministic revert, and a stable `AccountMeta` set — exactly the
  "Routers (B1)" note below.
- **DFlow.** An aggregator that treats venues as interchangeable liquidity
  sources (AMMs, prop AMMs, order-book venues alike); becoming a routable
  venue is the same off-chain-quote + on-chain-swap shape as Jupiter, plus
  DFlow's own BD onboarding. It reads chain state, not `/v1`.
- **Titan.** A **meta-aggregator** sitting above Jupiter / DFlow / its own
  router, so a Dropset market becomes routable in Titan **for free once it
  is in Jupiter**. Direct integration is a private BD conversation with no
  published interface — scope it as "via Jupiter," not a separate build.

**Routers (B1) — take-ix limit semantics.** *(Canonical name: the take is
exposed on-chain, in the IDL, and in the SDK as the `swap` instruction;
"take" is this spec's role name for the same call.)*
The take instruction's limit
argument is a **regular 8-significant-figure `Price`** (the worst acceptable
fill). The reserved encodings `0x0` and `0xFFFFFFFF` are **no-bound sentinels**
(see **architecture.md → Price**), so a router must pass a real `Price` to get
slippage protection; the engine **deterministically reverts** when the book
cannot fill within it. **Partial fills commit** (emit their legs + a partial
take envelope); only a zero-fill-below-minimum reverts. The book lives in a
**single market account** (vaults are inside it), so the take is **not
account-hungry** — the per-hop budget item that matters for a multi-hop route is
the **+2 `event-cpi` accounts** the fill appends (see **architecture.md →
Events and emission**); if that ever binds, use the **bare-self-CPI (+1)**
escape hatch. The dominant cost is **CU** (the in-memory book reconstruction),
not account loading — consider a bounded top-of-book fast path if a full
reconstruction is too heavy, and validate each router's actual swap-CPI contract
before assuming a shared trait.

### MCP servers — orthogonal to listing

Whether a vendor ships a Model Context Protocol server is **mostly
irrelevant to getting listed**. As of writing, official MCPs exist for
CoinGecko (read-only market data), Jupiter (docs-only), and DFlow (agent /
trading tools); DEXScreener, DeFiLlama, and Birdeye have only community
wrappers (and "Birdeye MCP" collides with an unrelated reputation-SaaS
product — not the crypto-data vendor); Titan has none. Every one is a
**consume-data / agent-tooling** surface, not a submit-your-DEX channel,
so none shortcuts the integration work above. DFlow's is the only one with
operational weight, and only as a consumer of DFlow's routing — still not
a listing path for us.

**Price feed — optional, display-only, conditional (open: h).** The protocol
and indexer are oracle-free. A USD/FX feed is deferred **only if every
aggregator-listed market has a USD-stable quote leg**; any non-USD-quote
flagship requires a USD reference feed (GeckoTerminal needs USD reserve/volume
to display). The *quoting* feed used by makers lives in the **MM-bot layer**,
not the protocol or indexer.

______________________________________________________________________

## 5. API surface

- **One REST service.** The prototype ships **`/v1` only** (frontend + bots):
  vaults, positions, prices, book/depth, fills. Heavy derivation (APR, PnL,
  FX-pair grouping) stays client-side; `/v1` returns raw state + rollups.
- **Aggregator namespaces** (e.g. the GeckoTerminal endpoints) are **thin
  transforms over `/v1`**, authored against each vendor's **published spec** and
  validated against **the vendor's fixtures** (not self-authored guesses).
- **OpenAPI 3.1** describes the owned surface — primarily the frontend `/v1`
  contract, where Dropset owns both ends and can codegen typed clients + a mock
  server. It is not a spec the aggregators "consume."
- **Volume flavor per consumer:** raw fills to OHLCV/order-book vendors;
  self-trade-adjusted to DeFiLlama-style wash-sensitive consumers.

______________________________________________________________________

## 6. SDK

Two independent codegen spines:

**(A) IDL → clients.** `anchor-next` emits the IDL → **Codama** generates the
TypeScript (`@solana/kit`) and Rust clients (instruction builders, account /
event codecs, PDA helpers). CI discipline:

- **Pin `anchor-next` to an exact rev**, and **pin `anchor-cli` (the IDL
  generator) to the same rev** — otherwise the IDL-diff baseline drifts.
- **Gate CI on a diff of the full regenerated IDL.** `metadata.spec` is the
  Anchor **IDL-spec toolchain** version (constant once the rev is pinned) and
  `metadata.version` is the program crate version — so an equality check on a
  single metadata string catches **neither** field/type/discriminator changes;
  diff the whole document.

**(B) Consensus math + book math → WASM.** Two solana-free crates, split by
audit severity. **`dropset-math-core`** holds the consensus-critical
arithmetic that **runs on-chain** — the `Price` codec, the pure matcher math,
and the share/NAV/PnL kernels (the seeding `isqrt`, single-leg deposit
sizing, the pro-rata withdrawal slice, and the perf-fee + realized-PnL
formulas). The on-chain program depends on it directly (the `Price` codec via
the `idl` feature; the matcher + share kernels through thin `&mut Vault`
wrappers), so a bug here is an on-chain bug — keeping the crate small focuses
the must-audit surface. **`dropset-interface`** holds the off-chain-only
half — the account-layout mirror and the just-in-time book reconstruction
(`simulate_swap`) that decode raw account bytes for routers, the `/orderbook`
depth endpoint, and the WASM client. It **depends one-way on
`dropset-math-core`** (no cycle) and never runs on-chain, so a bug there
mis-predicts a quote rather than corrupting state — lower audit priority,
parity-pinned by the conformance vectors rather than by the engine running
the code.

Both compile to **WASM** for any TypeScript consumer that must run the exact
arithmetic. (The on-chain crates cannot target `wasm32`; a hand-mirrored port
is rejected.) `make wasm` builds one package over `dropset-interface` whose
`wasm` feature turns on math-core's, so it exports both the `simulate_swap`
binding and the `Price` codec bindings. Correctness is enforced by **shared
conformance vectors run in both Rust and TS CI**, generated from the engine's
reference traces; the vectors pin **both** the numeric outputs **and** the
chosen wire encoding (open: f) — or scope the first freeze to math and defer
wire-encoding conformance until (f) lands.

Three vector sets are checked in and replayed by both forks:
`price_vectors.json` (the `Price` codec), `quoting_vectors.json` (the
native↔relative book translation), and `share_vectors.json` (all six
share/NAV/PnL kernels above, so the frontend's NAV/PnL fallback reuses the
engine math instead of re-deriving it). The `Price` vectors also pin the
**reject contract** via an `encode_reject` set: both forks reject non-finite
and negative inputs (Rust `from_value` → `None`, TS `encodePrice` →
`RangeError`), and also any finite value so large that `value * 1e7`
overflows `u64` (e.g. `1e300`, `1e13`). The latter used to diverge — Rust's
saturating `f64`→`u64` cast normalized huge values to a bogus finite
`Price` while TS threw — but `from_value` now rejects at that boundary so
the two forks agree; the `share_vectors.json` `merge_entry_basis` set
likewise pins the `weighted_average` blend for structurally-valid prices
above the FX band (exact-bigint precision and `u128` saturation).

A fourth fixture, `simulate_swap_vectors.json`, is scoped narrower and is
**not** a both-forks set: it is replayed only by the interface crate's WASM
conformance test (`sdk/interface/tests/wasm_conformance.rs`, run under
`wasm-pack test --node`), pinning the compiled `simulate_swap` binding to the
native matcher — closing the wasm-binding == native == engine chain — rather
than exercising the shared math-core kernels the three above do.

On-chain CPI builders (instruction builders + account layouts for a `no_std`,
entrypoint-free CPI parser, shared with the engine and any router doing an
on-chain integration) remain a separate future concern; the SDK above is the
off-chain artifact and CPIs do not live there.

______________________________________________________________________

## 7. Open decisions affecting this contract

This is the open-decisions list for the consumer contract — the **(open:
x)** markers elsewhere in this document point here. (Letters are
non-contiguous because they share a labelling scheme with protocol-level
decisions that do not affect this contract.)

- **(a)** Emit model — **resolved: per-leg emit.** Each matched leg is its own
  `emit_cpi!` `FillEvent` (see §1), so `event_ordinal` is the inner-instruction
  index and the PK `(slot, txn_index, signature, event_ordinal)` is frozen.
- **(f)** Price/amount wire representation — shared by events, SDK, OpenAPI.
- **(h)** Price-feed conditional trigger — required once any non-USD-quote
  market is listed on an aggregator.
