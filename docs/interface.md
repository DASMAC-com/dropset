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

Split by **how each vendor ingests** — and whether the integration is in our
control:

| Consumer                              | Model                                                                                                                                                                               | Control                                               |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| **CoinGecko / GeckoTerminal**         | **Project-hosted adapter** conforming to GeckoTerminal's non-EVM on-chain DEX endpoint spec (`/latest-block`, `/asset`, `/pair`, `/events`), derived from `/v1`. Order-book shaped. | **Ours — fixtures-ready** (most listable Solana path) |
| **DEXScreener / Birdeye**             | **Vendor-side ingestion / partner request**: deliver a stable program ID + this spec + a reference decoder; the vendor builds the parser.                                           | **Out of our control — "live on vendor"**             |
| **DeFiLlama**                         | A `dimension-adapters` **JS PR** that calls `/v1`. Report **self-trade-adjusted `dailyVolume`** (DeFiLlama de-washes); OHLCV/order-book vendors get **raw** fills.                  | PR + their review                                     |
| **Routers — Jupiter / DFlow / Titan** | Off-chain **quoting Rust adapter (B1)** that reads market state and prices it; see §below.                                                                                          | Ours + each router's onboarding                       |
| **beethoven (B2)**                    | CPI taker-swap composability. **Post-MVP, blocked on an upstream swap-context extension** (named owner + fallback). **Does not gate B1.**                                           | External dependency                                   |
| **Dropset frontend**                  | Consumes `/v1` (vaults, positions, prices).                                                                                                                                         | Ours                                                  |
| **MM bots** (incl. the reference bot) | Read book/vault/fill state from `/v1`; drive quoting via the SDK.                                                                                                                   | Ours / community                                      |

**Routers (B1) — take-ix limit semantics.** The take instruction's limit
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
