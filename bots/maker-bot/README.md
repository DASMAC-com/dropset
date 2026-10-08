# dropset-maker-bot

The localnet market-maker for the FX-stablecoin demo. A supervisor over
many `<token>/USDC` markets — the non-USD FX stablecoins in
`config::MARKETS` (EURC, VCHF, TGBP, ZARP, MXNe, XSGD, IDRX, and the MVP
pair additions AUDD and CADC) — quoting on
the eCLOB per [`docs/market-making.md`](../../docs/market-making.md).
One shared leader quotes every market; each cycle the bot refreshes a
batched, tiered price feed, composes a per-market fair mid, and drives
the program's relative-quoting hot path (`set_reference_price`, with an
inventory skew) and cold path (`set_liquidity_profile`) under the spec's
inventory / peg / staleness kill switches.

## `DROPSET_DATABASE_URL` is required — the bot will not start without it

The intraday FX anchor is read from the shared market-data store, so the
connection string is a **startup requirement**, not an optional
telemetry nicety. Run the binary without it and it exits immediately
naming the variable.

Most paths supply it already and you will never notice: compose sets it
for the containerized maker, and the TUI defaults it to the localnet
store for the makers it spawns. The path that does notice is a bare
`cargo run -p dropset-maker-bot` from a shell that has not exported it.

```sh
DROPSET_DATABASE_URL='postgres://dropset:dropset@127.0.0.1:5432/dropset' \
  cargo run -p dropset-maker-bot -- --dry-run
```

Failing closed here is deliberate. The alternative is a maker that
starts, finds no intraday anchor, and quotes the MVP pairs off a daily
ECB fix — which is the risk the fail-closed posture exists to decline.
The same reasoning halts a running bot when the store goes silent; see
`HaltReason::PriceStoreUnavailable`.

## The tiered price feed

Each market's USD reference cascades through four sources, primary-first,
failing over on a stale or errored tier:

1. **CoinGecko** `/simple/price` — one batched call prices every token
   (the primary market feed).
1. **CoinMarketCap** `/public-api/v1/simple/price` — batched by numeric
   id over the **keyless public** route, so it needs no key and carries
   no monthly credit quota. The secondary, read when CoinGecko has no
   price for a market.
1. **ECB/Frankfurter** `/latest` — the keyless FX-rate tier: `USD/<ccy>`
   inverted to a USD-per-unit peg, a pure peg rate.
1. **Static** — a per-market constant, the last resort.

A live market price (tiers 1–2) quotes healthy; a peg-rate fallback
(tiers 3–4) runs the vault degraded, tightening the kill switches. When
a market price and a fresh FX rate coexist, the price is peg-checked
against the FX rate.

## Layout

- `config` — the spec's knobs and the `MARKETS` roster (each market's
  CoinGecko id, optional CoinMarketCap numeric id, FX currency, mock
  mint, decimals, and static peg), with defaults encoding the spec.
- `model` — the pure, unit-tested quoting logic: tiered feed parsing
  (`feeds`), per-market reference composition (`fair_mid`), the ladder
  builder, inventory valuation and skew, the update-cadence triggers,
  the kill-switch policy, and the stale-quote decision (`invalidate`).
- `context` / `chain` / `tasks` — per-market runtime state, on-chain I/O
  (market discovery, vault reads, the two quoting-path sends, the
  stale-quote kill stamp, the human↔atoms-ratio price conversion), and
  the supervisor tick loop.
- `quote_state` — the one fact that outlives the process: when each
  market's book was last correctly priced, persisted one file per market.

## Running

Prerequisites: a localnet `solana-test-validator` with the program
deployed and the demo markets bootstrapped and seeded (the `dropset-tui`
control plane does this — its bootstrap brings up all markets).

Dry run — poll the tiered feeds once and print the reference each market
*would* stamp, with no validator and no writes (the wiring check for
feed credentials). `--drop` suppresses a tier so the cascade to the next
one is observable:

```sh
cargo run -p dropset-maker-bot -- --dry-run
cargo run -p dropset-maker-bot -- --dry-run --drop coingecko --drop cmc
```

Live — discover the markets, fund the leader from the faucet, and drive
the supervisor loop:

```sh
cargo run -p dropset-maker-bot
```

Mainnet — quote the mainnet roster (EURC, AUDD, CADC on their real
mints) with the leader key from the secrets chain. Nothing is airdropped;
fund the leader first:

```sh
cargo run -p dropset-maker-bot -- --cluster mainnet \
  --rpc <mainnet-rpc-url> --ws <mainnet-ws-url>
```

Stopping the bot (SIGINT / SIGTERM) **pulls its liquidity** before the
process exits, with the same pair a kill-switch halt sends. A market
whose book is still live gets the kill stamp. A market whose profile is
not already zeroed then gets a zeroed profile. A failed send is logged
and the other markets still go down; the process then exits non-zero. A
second signal exits at once without finishing.

### Flags

- `--cluster <name>` — `localnet` (default) or `mainnet`. The genesis
  check runs both ways: localnet mode refuses every public cluster, and
  mainnet mode refuses anything that is not mainnet-beta.
- `--rpc <url>` — RPC endpoint (default `http://127.0.0.1:8899`;
  required in mainnet mode).
- `--ws <url>` — PubSub websocket for the fill-event subscription
  (default: derived from `--rpc`, swapping the scheme and using the RPC
  port + 1, so `8899` → `8900`). Required in mainnet mode: the
  derivation keeps only the host, dropping any API key a provider carries
  in the URL path or query.
- `--leader-key <path>` — localnet only: leader / quote-authority
  keypair (default `keys/EEEE.json`, the role key the bootstrap seeds
  every vault with). **Refused in mainnet mode**, where the key is the
  `dropset/maker-leader` secret instead (see Environment).
- `--dry-run` — poll feeds and print the intended quotes, then exit.
- `--drop <tier>` — dry-run only: suppress `coingecko`, `cmc`, or `fx`
  (repeatable) to watch the cascade fall through.

### Environment

Localnet mode needs nothing set: every feed in the cascade is keyless,
so the bot prices its whole roster with no secret configured.

Mainnet mode needs the leader key, resolved once at startup as the
canonical secret `dropset/maker-leader` (`feeds::secrets`): the
`DROPSET_MAKER_LEADER` environment variable first, then the
`dropset` item's `maker-leader` field in the 1Password vault
`DROPSET_OP_VAULT` names. The value is the keypair in `solana-keygen`'s
JSON byte-array form. It is never read from a file, and a key on the
committed `keys/` roster is refused.

## Notes and deferrals

- **Genesis-checked, in both directions.** On startup the bot reads the
  cluster's genesis hash before it loads any key. Localnet mode refuses
  mainnet-beta, devnet and testnet. Its airdrop needs the localnet
  faucet and its committed leader key must never sign on a public
  cluster. Mainnet mode refuses anything that is not mainnet-beta. The
  check is keyed on the genesis hash, not the RPC host, so a localnet on
  any address still passes while a port-forward to a public cluster is
  caught.

- **One supervisor, one leader.** Both modes run all markets from one
  process under one quote-authority. The delegated
  per-market `quote_authority` model (one hot key per market) is the
  devnet/mainnet promotion's concern.

- **`FreezeVault` is admin-only.** The bot signs only as the leader, so
  the hard kill-switch triggers (peg breach, TVL floor, critical
  imbalance) **halt quoting** (zero the profile, kill the resting book)
  and alert for human review rather than calling the irreversible,
  admin-gated `FreezeVault` autonomously.

- **Stale quotes are killed, not left to expire.** Zeroing the profile
  stops the next flush; it doesn't touch levels already resting, which
  stay matchable until one of their two deadlines passes. Expiry is now
  dual-domain — a wall bound measured from the quote's `quote_unix`
  datum, and a slot bound from its `quote_slot` — so a halt no longer
  freezes the countdown the way slot-only expiry did. But a cap is not a
  policy: the deepest tier still runs ~48 min. Whenever nobody is
  refreshing the
  reference — a restart, a halted chain, dead feeds, a kill-switch halt —
  the bot stamps `price = 0` through the ordinary hot path. Matching
  skips a vault whose reference fails `has_valid_reference_price()`, so
  that one cheap instruction takes the whole book dark while leaving the
  ladder intact; the next live quote re-arms the same shape. On startup
  this runs **before** the first quote of the run, since takers can hit
  the old levels from the first block the bot is back.

  Age comes from the bot's own persisted timestamp
  (`.maker-bot/quote-state/<market>.json`, git-ignored), not from the
  chain: the vault records `quote_slot`, and slot arithmetic is precisely
  what a halt invalidates. A missing or unreadable record reads as stale.
  The default bound is 60 s — twice the reference heartbeat, so an
  ordinary restart doesn't churn the book, and ~2% of the deepest tier's
  ~50 min life. Clearing the state directory is safe; it just makes the
  next startup invalidate.

  On a halt the kill stamp goes **first**, ahead of zeroing the profile —
  only the stamp stops a taker, and a halt is when a stale price is most
  worth picking off.

  **Verification split.** The on-chain half is covered end-to-end by a
  program test (a zero reference takes the vault out of matching, leaves
  the ladder bytes untouched, and a plain re-quote refills it), and the
  staleness decision, the persistence, and the gate's early-outs are unit
  tested. The **startup ordering** — that the kill stamp is the run's
  first quoting transaction — has no automated coverage: this crate is
  localnet-only and has no validator harness, so that claim is verified by
  running `make demo` against a bootstrapped localnet.

- **Fill detection** subscribes to the program's `emit_cpi!`
  `FillEvent`s (production-fidelity path): a dedicated thread runs a
  `logsSubscribe` and reads the events out of each transaction's inner
  instructions via `getTransaction`. One subscription covers every
  market the leader quotes; the supervisor routes each fill to its
  market by `event.market`. The per-market vault read reconciles that
  belief (catching a missed fill or external flow) and is the sole
  signal in the fallback path when no subscription is attached.
