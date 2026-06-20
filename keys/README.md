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
`Keypair::read_keypair_file`.

> [!WARNING]
> These are throwaway **localnet-only** keys. Their secret keys are
> committed in plain text, so anyone can sign for them. Never fund a
> vanity address here on devnet or mainnet, and never reuse one for
> anything that holds real value.

## The set

The four-character prefix is the identifier. Role assignment is by
convention — the consumer (e.g. the dropset TUI) decides which key
plays which part:

| File        | Address | Conventional role                 |
| ----------- | ------- | --------------------------------- |
| `AAAA.json` | `AAAA…` | admin / payer / upgrade authority |
| `BBBB.json` | `BBBB…` | market maker (vault leader)       |
| `CCCC.json` | `CCCC…` | market maker                      |
| `DDDD.json` | `DDDD…` | taker                             |
| `EEEE.json` | `EEEE…` | taker                             |
| `FFFF.json` | `FFFF…` | spare participant                 |

## Regenerating

Each key was ground with the Solana CLI:

```sh
solana-keygen grind --starts-with AAAA:1
```

then renamed from the generated `<address>.json` to its prefix name. A
reground key has a different address, so only regrind if a key is
compromised or the scheme changes — downstream tooling and any saved
localnet ledger reference these exact addresses.
