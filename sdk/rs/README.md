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
- **Quoting** (`quoting`) — the native-CLOB direction: translate a full book
  of absolute price levels and atom sizes into the relative `profile_bytes`
  argument `set_liquidity_profile` expects. This is where the profile builder
  lives, which the top-level SDK README sends readers here to find.
- **Events** (`events`) — decode the `emit_cpi!` event payloads an indexer
  extracts from inner instructions. `try_decode_event_payload` is the strict
  form: it requires the body to be fully consumed and reports
  `DecodeError::TrailingBytes` otherwise, so an on-chain field addition
  surfaces as a decode failure rather than as a silently narrower record.
  `decode_event_payload` is the same check with the reason discarded.
- **Time** (`time`) — the slot and wall-clock domains used to judge level
  expiry, which is dual-domain (a level is live only while both deadlines
  hold).

## Features

- `fetch` — async account-fetch helpers in the generated client (pulls
  `solana-client`).
- `serde` — `serde` derives on the generated types.
- `anchor` / `anchor-idl-build` — known-but-empty flags for the Codama
  anchor-compat gates; the anchor path is not shipped.

## License

Apache-2.0
