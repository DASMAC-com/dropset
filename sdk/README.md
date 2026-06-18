# Dropset SDK

Off-chain clients for the Dropset eCLOB program, plus the shared book math.
Implements the SDK section of [`docs/interface.md`](../docs/interface.md).

## Layout

```text
sdk/
  idl/dropset.json     Checked-in Anchor IDL (regenerate: `make idl`)
  codama/              Codegen: IDL -> TS + Rust clients (`make sdk`)
  ts/                  @dropset/sdk — @solana/kit client + Price/quoting
  rs/                  dropset-sdk — Rust client + router adapters
  math-core/           dropset-math-core — consensus math: Price codec,
                       matcher arithmetic, share/NAV/PnL kernels;
                       solana-free, runs on-chain (+ WASM)
  interface/           dropset-interface — off-chain account-layout mirror
                       + book simulator; depends on math-core (+ WASM)
  conformance/         Cross-language vectors (Rust + TS both verify)
```

## Two spines

**A. IDL → clients (Codama).** `anchor idl build` emits the IDL; Codama
generates the TypeScript (`@solana/kit`) and Rust clients — instruction
builders, account/event codecs, PDA helpers. Regenerate with
`make idl && make sdk`. Two codegen fix-ups live in `codama/generate.mjs`:
`Price` is remapped to its real `u32` wire form (the on-chain type isn't
`IdlType`, so it surfaces as a fieldless struct), and
`set_liquidity_profile`'s `profile_bytes` is restored to `[u8; 160]`
(anchor-next can't const-eval `PROFILE_BYTES`).

**B. Book math (`math-core` + `interface`).** The consensus arithmetic —
the `Price` codec, the pure matcher math, and the share/NAV/PnL kernels —
lives once in the solana-free `dropset-math-core` crate and **runs
on-chain**; the program depends on it directly so the engine and every
off-chain consumer share one audited copy. The off-chain account-layout
mirror + just-in-time book reconstruction, which can't be derived from the
IDL (the `Vault` slab is opaque to it), live in `dropset-interface`, which
depends one-way on math-core. The Rust SDK re-exports both. The TS client
currently ships a thin hand-written mirror of the `Price` codec +
native-quoting math, kept in exact integer lockstep with the Rust engine by
the conformance vectors below. `make wasm` compiles `dropset-interface`
(which turns on math-core's `wasm` feature) to one WASM package exporting
both binding sets; wiring it into `@dropset/sdk` to retire the TS mirror
(interface.md §6B) is a tracked follow-up.

## Conformance

`sdk/conformance/price_vectors.json` is generated from the Rust reference
(`cargo run -p dropset-math-core --example gen_conformance`) and verified
in **both** languages — Rust (`math-core/tests/conformance.rs`) and TS
(`ts/src/conformance.test.ts`). Run all SDK tests with `make sdk-test`.
Scope today is the `Price` codec + ratio math (`quote_for_base` /
`base_for_quote`); `simulate_swap` conformance (needs serialized market
fixtures) is a follow-up.

## Quoting: native vs relative

The program quotes *relatively* (a reference price + ppm offsets / bps
sizes). Both SDKs add the **native CLOB** direction — specify a full
absolute-price book and translate it to the on-chain `LiquidityProfile`:
`quoting::NativeBook::to_profile` (Rust) / `nativeBookToProfileBytes` (TS).

## Router adapters (`rs/src/adapters`)

A router-agnostic core (`adapters::amm::DropsetAmm`: load → quote via
`simulate_swap` → swap instruction, **no network calls**) with thin
per-router mappings:

- **jupiter** / **dflow** — the `Amm` trait. Drop-in upstream impls are
  blocked on each router forking our SDK + adding a Dropset variant to their
  closed `Swap` enum, and on solana-crate version skew (boundary byte
  conversion). Tracked: ENG-442 (Jupiter), ENG-443 (DFlow).
- **titan** — the `TradingVenue` trait; closest to drop-in (no closed enum).

beethoven is **not** an off-chain adapter — it's an on-chain Pinocchio CPI
integration, so per interface.md §6 it belongs in the on-chain
`dropset-interface` crate, not here. Blocked on a swap-context extension;
tracked in ENG-444.

## Verification

The generated clients build instruction data that matches the program by
construction (the IDL is generated from it). The hand-written book math is
verified two ways: cross-language conformance vectors (above), and
`programs/dropset/tests/sdk_conformance.rs`, which runs the **real** `swap`
in litesvm and asserts the SDK's `MarketView` decode + `simulate_swap`
prediction equal the on-chain realized amounts (it caught the sector
alignment in `VAULT_ALIGN`). That test runs under `make test`.
