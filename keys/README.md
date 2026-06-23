<!-- cspell:word devnet -->

<!-- cspell:word keygen -->

<!-- cspell:word keypairs -->

<!-- cspell:word localnet -->

<!-- cspell:word vanity -->

# Localnet vanity keypairs

Pre-ground vanity keypairs for the localnet roles, committed so a
localnet run is deterministic: the same recognizable addresses turn up
every time, in the TUI and in the explorer.

Each file is a standard Solana CLI keypair — a 64-byte JSON array (the
32-byte secret followed by the 32-byte public key). Inspect one with
`solana address -k keys/AAAA.json`, or load it in Rust with
`solana_keypair::read_keypair_file`.

> [!WARNING]
> These are throwaway **localnet-only** keys. Their secret keys are
> committed in plain text, so anyone can sign for them. Never fund a
> vanity address here on devnet or mainnet, and never reuse one for
> anything that holds real value.

## The set

The four-character prefix is the identifier. Roles are assigned by
convention, in the order the localnet bootstrap introduces them:

| File        | Address | Conventional role                   |
| ----------- | ------- | ----------------------------------- |
| `AAAA.json` | `AAAA…` | the dropset program ID              |
| `BBBB.json` | `BBBB…` | admin 1 — payer / upgrade authority |
| `CCCC.json` | `CCCC…` | admin 2                             |
| `DDDD.json` | `DDDD…` | registrant                          |
| `EEEE.json` | `EEEE…` | vault leader                        |
| `FFFF.json` | `FFFF…` | taker                               |

`AAAA.json` is the **program keypair**: it is copied into
`target/deploy/dropset-keypair.json` at build time (the `program-keypair`
Makefile target) so `declare_id!` and anchor's build-time program-ID
check agree. The rest are signer accounts the TUI and bots fund and
drive. Need more participants (extra takers or makers)? Grind the next
prefix (`GGGG`, `HHHH`, …) into this directory.

## The mock token mints

The localnet market bootstrap also uses two **fixed mint keypairs**, so the
traded pair — and therefore the market PDA, seeded on `[base, quote]` —
lands at the same address on every run. Both are 6-decimal SPL mints whose
mint authority is the localnet wallet, created fresh against each new
validator:

| File        | Address | Conventional role         |
| ----------- | ------- | ------------------------- |
| `CADC.json` | `CADC…` | base mint (mock CAD coin) |
| `USDC.json` | `USDC…` | quote mint (mock USDC)    |

These are named in the bootstrap's pair config (`tui/src/market.rs`,
`MOCK_CADC_USDC`); the vault leader is `EEEE.json` above. Add another pair
by grinding two more mint prefixes and adding a `PairConfig` that names
them.

## Regenerating

Each key was ground with the Solana CLI:

```sh
solana-keygen grind --starts-with AAAA:1
```

then renamed from the generated `<address>.json` to its prefix name. A
reground key has a different address, so only regrind if a key is
compromised or the scheme changes — downstream tooling and any saved
localnet ledger reference these exact addresses.
