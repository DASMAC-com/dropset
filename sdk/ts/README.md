# @dropset/sdk

TypeScript client for the
[Dropset](https://github.com/DASMAC-com/dropset) eCLOB program, built on
[`@solana/kit`](https://github.com/anza-xyz/kit) — for frontend apps,
market makers, routers, and indexers.

- **Generated client** (`./generated`) — the Codama-generated `@solana/kit`
  client built from the Anchor IDL: instruction builders, account & event
  codecs, PDA helpers, and program constants.
- **`Price` codec** — the bits ↔ decimal conversion for the on-chain `u32`
  decimal floating-point comparison key, which the IDL exposes only as raw
  `u32` bits. Used to display prices and to build `set_reference_price` /
  `swap` arguments.
- **Quoting** — the native-CLOB direction: translate a full book of
  absolute price levels and atom sizes into the relative `profile_bytes`
  arg `set_liquidity_profile` expects. The TypeScript mirror of
  `dropset-sdk`'s `quoting` module.
- **Share / NAV / PnL kernels** — the scalar deposit, withdraw, and
  perf-fee formulas that run on-chain, mirrored in `bigint` so the frontend
  can preview NAV and share value without an indexer. Pinned to the engine
  by the cross-language conformance vectors.

## Usage

```ts
import { encodePrice, getSwapInstruction } from "@dropset/sdk";
```

The root export re-exports the generated client alongside the hand-written
`Price`, quoting, and share modules; the generated client is also available
on its own at `@dropset/sdk/generated`. Regenerate the `generated/` tree
with `make sdk` after `make idl`.

## License

Apache-2.0
