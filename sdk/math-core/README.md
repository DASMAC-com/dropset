# dropset-math-core

Solana-free, consensus-critical eCLOB arithmetic for the
[Dropset](https://github.com/DASMAC-com/dropset) program.

The on-chain crates can't target `wasm32` and a hand-mirrored TypeScript
port is rejected, so the consensus math lives here exactly once and is
consumed directly by the on-chain program **and** the Rust SDK, and
compiled to WASM for the TypeScript client. Every consumer runs
byte-identical code.

## Contents

- **`Price` codec** — the canonical `u32` price encoding, where unsigned
  integer order matches price order.
- **Matcher arithmetic** — the pure ratio math (`quote_for_base` /
  `base_for_quote`) the on-chain engine fills against.
- **Share / NAV / PnL kernels** — the vault accounting primitives.

Correctness is pinned to the on-chain engine by the shared conformance
vectors under [`sdk/conformance`](../conformance).

## Features

- `wasm` — `wasm-bindgen` exports of the `Price` codec for the TS client.
- `idl` / `idl-build` — Anchor `IdlType` derive on `Price`, for the
  on-chain program's IDL build. **Not** solana-free (pulls
  `anchor-lang-v2`); off by default and never combined with `wasm`.

## License

Apache-2.0
