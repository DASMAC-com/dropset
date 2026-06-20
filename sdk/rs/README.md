# dropset-sdk

Off-chain Rust client and book math for the
[Dropset](https://github.com/DASMAC-com/dropset) eCLOB program — for market
makers, routers, and indexers.

- **Generated client** (`src/generated`) — the Codama-generated client
  built from the Anchor IDL: instruction builders, account/event codecs,
  and PDA helpers.
- **Book math** — re-exports the shared, solana-free consensus arithmetic
  ([`dropset-math-core`](../math-core)) plus the off-chain account-layout
  mirror and book simulator ([`dropset-interface`](../interface)), which
  the IDL can't describe (the `Vault` slab is opaque to it).
- **Router adapters** (`src/adapters`) — a router-agnostic core
  (`adapters::amm::DropsetAmm`: load → quote via `simulate_swap` → swap
  instruction, no network calls) with thin per-router mappings (Jupiter,
  DFlow, Titan).

## Features

- `fetch` — async account-fetch helpers in the generated client (pulls
  `solana-client`).
- `serde` — `serde` derives on the generated types.
- `anchor` / `anchor-idl-build` — known-but-empty flags for the Codama
  anchor-compat gates; the anchor path is not shipped.

## License

Apache-2.0
