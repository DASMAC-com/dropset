<!-- cspell:word keygen -->

<!-- cspell:word keypairs -->

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

| File        | Address | Conventional role                          |
| ----------- | ------- | ------------------------------------------ |
| `AAAA.json` | `AAAA…` | the dropset program ID                     |
| `BBBB.json` | `BBBB…` | admin 1 — payer / upgrade / mint authority |
| `CCCC.json` | `CCCC…` | admin 2                                    |
| `DDDD.json` | `DDDD…` | registrant                                 |
| `EEEE.json` | `EEEE…` | vault leader                               |
| `FFFF.json` | `FFFF…` | taker                                      |

`AAAA.json` is the **program keypair**: it is copied into
`target/deploy/dropset-keypair.json` at build time (the `program-keypair`
Makefile target) so `declare_id!` and anchor's build-time program-ID
check agree. The rest are signer accounts the TUI and bots fund and
drive. Need more participants (extra takers or makers)? Grind the next
prefix (`GGGG`, `HHHH`, …) into this directory.

## The mock token mints

The localnet market bootstrap also uses **fixed mint keypairs**, so each
traded pair — and therefore its market PDA, seeded on `[base, quote]` —
lands at the same address on every run. Every demo market is
`<token>/USDC`, so USDC is the shared quote and each FX stablecoin gets
its own base mint. Their mint authority is the localnet admin wallet
(`BBBB.json`), created fresh against each new validator. The base mints'
decimals match the real tokens so the localnet plumbing exercises the same
per-market decimal handling the devnet/mainnet promotion will:

| File        | Address | Dec | Conventional role             |
| ----------- | ------- | --- | ----------------------------- |
| `USDC.json` | `USDC…` | 6   | shared quote mint (mock USDC) |
| `EURC.json` | `EURC…` | 6   | base — mock EURC (EUR)        |
| `VCHF.json` | `VCHF…` | 9   | base — mock VCHF (CHF)        |
| `TGBP.json` | `TGBP…` | 9   | base — mock TGBP (GBP)        |
| `ZARP.json` | `ZARP…` | 6   | base — mock ZARP (ZAR)        |
| `MXNe.json` | `MXNe…` | 9   | base — mock MXNe (MXN)        |
| `XSGD.json` | `XSGD…` | 6   | base — mock XSGD (SGD)        |
| `idrx.json` | `idrx…` | 2   | base — mock IDRX (IDR)        |

These are named in the bootstrap's pair roster (`tui/src/market.rs`,
`PAIRS`) and the maker-bot's market roster (`bots/maker-bot` →
`config::MARKETS`); the vault leader is `EEEE.json` above. IDRX grinds as
lowercase `idrx` because the base58 alphabet has no capital `I`. These
**mock** mints are localnet-only — devnet/mainnet use the real token
mints. Add another pair by grinding one more base-mint prefix and adding
it to both rosters.

## Regenerating

Each key was ground with the Solana CLI:

```sh
solana-keygen grind --starts-with AAAA:1
```

then renamed from the generated `<address>.json` to its prefix name. A
reground key has a different address, so only regrind if a key is
compromised or the scheme changes — downstream tooling and any saved
localnet ledger reference these exact addresses.
