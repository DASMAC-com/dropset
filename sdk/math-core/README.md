# dropset-math-core

Solana-free, consensus-critical eCLOB arithmetic for the
[Dropset](https://github.com/DASMAC-com/dropset) program.

The on-chain crates can't target `wasm32` and a hand-mirrored TypeScript
port is rejected, so the consensus math lives here exactly once and is
consumed directly by the on-chain program **and** the Rust SDK, and
compiled to WASM for the TypeScript client. Every consumer runs
byte-identical code.

## Contents

- **`Price` codec** (`price`) — the canonical `u32` price encoding, where
  unsigned integer order matches price order. The ratio math
  (`quote_for_base` / `base_for_quote`) lives here too, as methods on
  `Price`.
- **Matcher arithmetic** (`matching_math`) — the pure fee and fill
  arithmetic the on-chain engine matches with, including the platform-fee
  computation and its rounding.
- **Clock** (`clock`) — the level-expiry domains. Expiry is **dual-domain**:
  a level is live only while *both* its slot deadline and its wall-clock
  deadline hold, and neither domain alone is sufficient. Every off-chain
  consumer that judges liveness needs this module.
- **Share / NAV / PnL kernels** (`share`) — the vault accounting primitives.

Correctness is pinned to the on-chain engine by the shared conformance
vectors under [`sdk/conformance`](../conformance).

## Features

- `wasm` — `wasm-bindgen` exports of the `Price` codec for the TS client.
- `idl` / `idl-build` — Anchor `IdlType` derive on `Price`, for the
  on-chain program's IDL build. **Not** solana-free (pulls
  `anchor-lang-v2`); off by default and never combined with `wasm`.

## License

Apache-2.0
