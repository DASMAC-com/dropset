# Dropset SDK

Off-chain clients for the Dropset eCLOB program, plus the shared book math.
Implements the SDK section of [`docs/interface.md`](../docs/interface.md).

## Layout

```
sdk/
  idl/dropset.json     Checked-in Anchor IDL (regenerate: `make idl`)
  codama/              Codegen: IDL -> TS + Rust clients (`make sdk`)
  ts/                  @dropset/sdk — @solana/kit client + Price/quoting
  rs/                  dropset-sdk — Rust client + router adapters
  price-core/          dropset-price-core — solana-free math (+ WASM)
  conformance/         Cross-language vectors (Rust + TS both verify)
```

## Two spines

**A. IDL → clients (Codama).** `anchor idl build` emits the IDL; Codama
generates the TypeScript (`@solana/kit`) and Rust clients — instruction
builders, account/event codecs, PDA helpers. Regenerate with `make idl &&
make sdk`. Two codegen fix-ups live in `codama/generate.mjs`: `Price` is
remapped to its real `u32` wire form (the on-chain type isn't `IdlType`, so
it surfaces as a fieldless struct), and `set_liquidity_profile`'s
`profile_bytes` is restored to `[u8; 160]` (anchor-next can't const-eval
`PROFILE_BYTES`).

**B. Book math (`price-core`).** The `Price` codec and just-in-time book
reconstruction can't be derived from the IDL (the `Vault` slab is opaque to
it). They live once in the solana-free `dropset-price-core` crate, used
directly by the Rust SDK and compiled to **WASM** (`make wasm`) for the TS
client — instead of each language hand-mirroring the engine. Correctness is
pinned by the conformance vectors below.

## Conformance

`sdk/conformance/price_vectors.json` is generated from the Rust reference
(`cargo run -p dropset-price-core --example gen_conformance`) and verified
in **both** languages — Rust (`price-core/tests/conformance.rs`) and TS
(`ts/src/conformance.test.ts`). Run all SDK tests with `make sdk-test`.
Scope today is the `Price` math; `simulate_swap` conformance (needs
serialized market fixtures) is a follow-up.

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
- **beethoven** — on-chain Pinocchio CPI; documentation-only here (CPIs
  belong in the on-chain `dropset-interface` crate per interface.md §6),
  blocked on a swap-context extension. Tracked: ENG-444.

## Verification

The generated clients build instruction data that matches the program by
construction (the IDL is generated from it). The hand-written book math is
verified two ways: cross-language conformance vectors (above), and
`programs/dropset/tests/sdk_conformance.rs`, which runs the **real** `swap`
in litesvm and asserts the SDK's `MarketView` decode + `simulate_swap`
prediction equal the on-chain realized amounts (it caught the sector
alignment in `VAULT_ALIGN`). That test runs under `make test`.
