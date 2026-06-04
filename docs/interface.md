<!-- cspell:word eclob -->

<!-- cspell:word geyser -->

<!-- cspell:word coingecko -->

<!-- cspell:word dexscreener -->

<!-- cspell:word defillama -->

<!-- cspell:word birdeye -->

<!-- cspell:word geckoterminal -->

<!-- cspell:word codama -->

<!-- cspell:word anchorpy -->

<!-- cspell:word borsh -->

<!-- cspell:word pyth -->

<!-- cspell:word beethoven -->

<!-- cspell:word priceNative -->

<!-- cspell:word txnIndex -->

<!-- cspell:word dailyVolume -->

<!-- cspell:word mev -->

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

**Status.** The program is currently an init-only skeleton; this spec defines
the target contract. Items marked **(open: x)** depend on a decision tracked in
the plan's open-decisions list.

______________________________________________________________________

## 1. Event contract

Events are emitted on cold paths only; the hot path
(`SetReferencePrice`/`SetLiquidityProfile`) emits nothing (see
**architecture.md → Events and emission**). A single take can match many levels
across many vaults; **every leg is recorded — full fidelity, never dropped**,
which is why fills ride as `emit_cpi!` inner-instruction data rather than logs
(`sol_log_data` silently truncates past the ~10 KB-per-tx ceiling, which a
level-blasting sweep would hit). A fill is emitted **per matched
`(vault, level)`** plus one **take-level summary**.

### `MakerFill` — one per matched (vault, level)

| field                                            | meaning                                                                                                         |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `leg_price`                                      | the level's absolute `Price` this leg filled at                                                                 |
| `fill_base`                                      | base atoms moved on this leg                                                                                    |
| `fill_quote`                                     | quote atoms moved on this leg                                                                                   |
| `vault`                                          | the matched vault — a **stable id**, not the reusable sector index                                              |
| `leader`                                         | the vault's economic owner (carried directly — sectors are reused, so an index is not a stable attribution key) |
| `quote_authority`                                | the delegated quoting wallet (for off-chain wash clustering)                                                    |
| `level_idx`                                      | which `LiquidityProfile` level                                                                                  |
| `nonce`                                          | the level's stamped `market.nonce` (price-time priority)                                                        |
| `post_fill_vault_base` / `post_fill_vault_quote` | vault inventory after this leg                                                                                  |

### Take-level envelope — one per take

| field                                  | meaning                    |
| -------------------------------------- | -------------------------- |
| `market`                               | the market account         |
| `taker`                                | the transaction initiator  |
| `side`                                 | buy/sell (base)            |
| `total_fill_base` / `total_fill_quote` | summed across all legs     |
| `taker_fee_atoms`                      | protocol taker fee charged |
| `signature` / `slot` / `txn_index`     | transaction coordinates    |

`avg_price` is **derived** (`total_fill_quote / total_fill_base`), never emitted.

### Lifecycle / liquidity events

- **`Deposit` / `Withdraw`** — inventory deltas (join/exit liquidity; feed TVL
  and per-depositor cost-basis).
- **`OpenVault`** — a new vault/leader enters a market.
- **`Realize`** — performance-fee accrual (leader economics).

### Dedupe / primary key

The event primary key is **`(slot, txn_index, signature, event_ordinal)`**.
`event_ordinal` is assigned in **heap-pop (match) order**, independent of flush
boundaries; `post_fill_vault_*` is the snapshot after that leg. Its concrete
source depends on the emit model **(open: a)**:

- *per-leg emit* → the inner-instruction index within the transaction, counting
  **all** Dropset self-CPI inner instructions (stable across replay);
- *packed batch* → the leg index within the decoded batch.

**Locking the emit model is a prerequisite to freezing this key / writing any
rows.** Plain `(slot, txn_index, signature)` is insufficient: it collides across
the N legs of one take.

### Volume integrity

Self-trade / wash is **detected off-chain** (wallet clustering over the
`taker` / `leader` / `quote_authority` exposed on every leg) — there is no
on-chain wash flag (a fresh wallet defeats a signer check; no leader allowlist).
Publish **both raw and self-trade-adjusted** volume; never silently net.

______________________________________________________________________

## 2. Wire representation

An `emit_cpi!` event is a **self-CPI inner instruction** (not a `Program data:`
log). Its instruction data is:

```
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
(`event_authority` + `program`) on every emitting ix — **negligible on the fill**
(the whole book is one market account) but a real budget item for **routers**
(§4). If a router's multi-hop account budget ever binds, the escape hatch is a
**bare self-CPI** (+1 account: drop `event_authority`, keep `program`); origin is
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
event codecs, PDA helpers); **AnchorPy** is the Python path (spike against a
real `anchor-next` IDL, or defer — no listed consumer needs Python yet). CI
discipline:

- **Pin `anchor-next` to an exact rev**, and **pin `anchor-cli` (the IDL
  generator) to the same rev** — otherwise the IDL-diff baseline drifts.
- **Gate CI on a diff of the full regenerated IDL.** `metadata.spec` is the
  Anchor **IDL-spec toolchain** version (constant once the rev is pinned) and
  `metadata.version` is the program crate version — so an equality check on a
  single metadata string catches **neither** field/type/discriminator changes;
  diff the whole document.

**(B) Price / book math → WASM.** A **new solana-free `price-core` crate** (the
`Price` codec + book-reconstruction math) compiled to **WASM** for any
TypeScript/Python consumer that must run the exact arithmetic. (The on-chain
crates cannot target `wasm32`; a hand-mirrored port is rejected.) Correctness is
enforced by **shared conformance vectors run in both Rust and TS CI**, generated
from the engine's reference traces; the vectors pin **both** the numeric outputs
**and** the chosen wire encoding (open: f) — or scope the first freeze to math
and defer wire-encoding conformance until (f) lands.

CPIs themselves live in a separate **on-chain `dropset-interface` crate**
(`no_std`, entrypoint-free: instruction builders + account layouts + the
`price-core` math), shared bit-for-bit across the engine, the router quoting
adapters, the `/orderbook` depth endpoint, and any CPI parser. The SDK above is
the off-chain artifact; CPIs do not live there.

______________________________________________________________________

## 7. Open decisions affecting this contract

These are tracked in full in the plan; the consumer-facing ones:

- **(a)** Emit model (packed `FillBatch`-per-take vs per-leg) — sets
  `event_ordinal` provenance and the `#[event]` struct shape. **Blocks freezing
  the event PK.**
- **(f)** Price/amount wire representation — shared by events, SDK, OpenAPI.
- **(h)** Price-feed conditional trigger — required once any non-USD-quote
  market is listed on an aggregator.
