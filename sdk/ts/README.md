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
- **Market reader** — decode the on-chain market slab (opaque to the IDL)
  and reconstruct the resting order book.
  `fetchDropsetMarketView(rpc, address, { nowUnix, nowSlot? })` returns
  `{ header, bids, asks }`. It is **not** a single-account poll: it issues
  `getAccountInfo` and, unless `nowSlot` is pinned by the caller, a
  `getSlot` alongside it. Level expiry is **dual-domain** — a level is live
  only while *both* its slot and wall-clock deadlines hold — so `nowUnix` is
  required and `nowSlot` defaults to a chain read rather than to a local
  clock. The book itself comes from the **WASM binding compiled from
  `dropset-interface`**, not from a TypeScript re-implementation: the
  hand-mirrored slab offsets and matching logic that used to live here were
  deliberately deleted, because restating the on-chain layout in a second
  language let it drift silently as the `Vault` layout grew. There is now one
  implementation of the book, and it is the one the chain runs.
- **Share / NAV / PnL kernels** — the scalar deposit, withdraw, and
  perf-fee formulas that run on-chain, mirrored in `bigint` so the frontend
  can preview NAV and share value without an indexer. Pinned to the engine
  by the cross-language conformance vectors.

## Usage

```ts
import { encodePrice, getSwapInstruction } from "@dropset/sdk";
```

The root export re-exports the generated client alongside every hand-written
module — `clock`, `dflow`, `events`, `market`, `price`, `quoting`, `route`,
`router`, `share`, and `simulate`. Note in particular the **simulator**
(`simulate`) and the **router** (`router` / `route`), the package's headline
quoting path, which earlier revisions of this list omitted. The generated
client is also available on its own at `@dropset/sdk/generated`. Regenerate
the `generated/` tree with `make sdk` after `make idl`.

## License

Apache-2.0
