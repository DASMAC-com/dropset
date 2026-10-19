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
| `EEEE.json` | `EEEE…` | vault leader and quote authority           |
| `FFFF.json` | `FFFF…` | taker                                      |

`AAAA.json` is the **program keypair**: it is copied into
`target/deploy/dropset-keypair.json` at build time (the `program-keypair`
Makefile target) so `declare_id!` and anchor's build-time program-ID
check agree. The rest are signer accounts the TUI and bots fund and
drive. Need more participants (extra takers or makers)? Grind the next
prefix (`GGGG`, `HHHH`, …) into this directory.

## Mainnet signing keys

No mainnet signing key lives here, and the TUI refuses any key in this
directory on mainnet, since its secret is public. The mainnet TUI reads one
1Password reference per key from its own git-ignored keys file,
`tui/keys.env`. To start one, copy the committed placeholder file
`tui/keys.env.example`. It holds three keys:

- the admin wallet
- the vault leader (Lead), the custody key, which the TUI resolves only
  when a custody step first needs it
- the vault quote authority (Quote), which signs the ladder and the
  re-centre, and which create-vault sets on every new vault

On localnet `EEEE.json` is both leader and quote authority.

That file is one of the repo's two secrets-reference homes. The other is
the localnet collectors enclave, `infra/localnet/secrets.local.env`, which
holds the data stack's credentials (`docs/data-feeds.md` §12). Each keeps
references only; the values stay in 1Password.

## The mainnet program id

`dropset-mainnet.pubkey` is the one non-localnet key here, and it is
**public-key only**: the mainnet program id (`FXv1…`). Its keypair is
held in 1Password and never enters the repo, a build, or an agent
session; only the publish step reads it, by its 1Password reference.

The program crate's `mainnet` feature selects this id's `declare_id!`
(`make program-mainnet`, artifact `target/deploy/dropset_mainnet.so`),
and the SDKs expose it as `DROPSET_MAINNET_ID` (Rust) and
`DROPSET_MAINNET_PROGRAM_ADDRESS` (TS), resolved per cluster through
`Deployment::program_id` / `dropsetProgramAddress`. Parity tests in both
SDKs pin those copies to this file, so changing the id means editing the
file, the feature-gated `declare_id!` and both SDK constants together.

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
| `AUDD.json` | `AUDD…` | 6   | base — mock AUDD (AUD)        |
| `CADC.json` | `CADC…` | 6   | base — mock CADC (CAD)        |

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

### Mainnet keys

Mainnet keys are ground by the operator with `grind_key.py`, never by an
agent session, so no secret ever enters a session or a checkout. One
command per key, taking its vanity prefix:

```sh
python3 keys/grind_key.py <Prefix>
```

The prefix match is case-sensitive. Each run writes one file to
`~/Desktop/dropset-vanity/` (override with `--out-dir`), outside every
checkout: `<pubkey>.txt`, mode 600, whose entire contents are the base58
encoding of the 64-byte secret keypair, the form Phantom imports and
exports. The keygen JSON is deleted, and the script prints only the
public key, the path and the byte count. If iCloud's "Desktop &
Documents Folders" sync is on, pass a non-synced `--out-dir` instead, or
the secret is uploaded the moment it is written.

Verify each file with a Phantom round trip before storing it:

1. Import the file's contents into Phantom as a private key, and confirm
   the address Phantom shows equals the file name's public key.
1. Export that account's private key from Phantom, and confirm it equals
   the file's contents byte for byte.

Then put the text file into 1Password and delete it from the Desktop.
A rotation is the same command. Only the program id's public half is
committed (`dropset-mainnet.pubkey`, see above); every other mainnet key
is a runtime input and never enters the repo.
