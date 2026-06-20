# dropset-interface

Solana-free, off-chain account-layout mirror and eCLOB book simulator for
the [Dropset](https://github.com/DASMAC-com/dropset) program, built on
[`dropset-math-core`](../math-core).

- **`layout`** mirrors the on-chain market account so a client can
  zero-copy decode the `Vault` slab the IDL can't describe.
- **`matching::simulate_swap`** reconstructs the book and simulates a fill,
  for routers and depth endpoints.

Both consume the consensus arithmetic from `dropset-math-core`, so the
simulator runs the exact same numbers as the on-chain engine — pinned by
the shared conformance vectors under [`sdk/conformance`](../conformance).
Nothing here runs on-chain: a bug mis-predicts a quote rather than
corrupting state.

The dependency is one-way — this crate depends on `dropset-math-core`,
never the reverse.

## Features

- `simulate` (default) — off-chain book reconstruction
  (`matching::simulate_swap`).
- `wasm` — `wasm-bindgen` exports for the TypeScript client; a single
  `wasm-pack` build over this crate emits both this crate's `simulate_swap`
  binding and the `Price` codec bindings forwarded from `dropset-math-core`.

## License

Apache-2.0
