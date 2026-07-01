# dropset-maker-bot

The localnet market-maker for the FX-stablecoin demo. A supervisor over
many `<token>/USDC` markets — the seven non-USD FX stablecoins in
`config::MARKETS` (EURC, VCHF, TGBP, ZARP, MXNe, XSGD, IDRX) — quoting on
the eCLOB per [`docs/market-making-mvp.md`](../../docs/market-making-mvp.md).
One shared leader quotes every market; each cycle the bot refreshes a
batched, tiered price feed, composes a per-market fair mid, and drives
the program's relative-quoting hot path (`set_reference_price`, with an
inventory skew) and cold path (`set_liquidity_profile`) under the spec's
inventory / peg / staleness kill switches.

## The tiered price feed

Each market's USD reference cascades through four sources, primary-first,
failing over on a stale or errored tier:

1. **CoinGecko** `/simple/price` — one batched call prices every token
   (the primary market feed).
1. **CoinMarketCap** `/v2/cryptocurrency/quotes/latest` — batched by
   numeric id, keyed from `CMC_API_KEY`. The free tier's quota rules out
   a hot poll, so it is the secondary, polled only when CoinGecko is
   down.
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
  mint, decimals, and static peg), with defaults encoding the MVP.
- `model` — the pure, unit-tested quoting logic: tiered feed parsing
  (`feeds`), per-market reference composition (`fair_mid`), the ladder
  builder, inventory valuation and skew, the update-cadence triggers,
  and the kill-switch policy.
- `context` / `chain` / `tasks` — per-market runtime state, on-chain I/O
  (market discovery, vault reads, the two quoting-path sends, the
  human↔atoms-ratio price conversion), and the supervisor tick loop.

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

### Flags

- `--rpc <url>` — RPC endpoint (default `http://127.0.0.1:8899`).
- `--ws <url>` — PubSub websocket for the fill-event subscription
  (default: derived from `--rpc`, swapping the scheme and using the RPC
  port + 1, so `8899` → `8900`).
- `--leader-key <path>` — leader / quote-authority keypair (default
  `keys/EEEE.json`, the role key the bootstrap seeds every vault with).
- `--dry-run` — poll feeds and print the intended quotes, then exit.
- `--drop <tier>` — dry-run only: suppress `coingecko`, `cmc`, or `fx`
  (repeatable) to watch the cascade fall through.

### Environment

- `CMC_API_KEY` — CoinMarketCap API key for the secondary tier. Without
  it the CoinMarketCap fallback is skipped and the cascade goes
  CoinGecko → FX-rate → static.

## Notes and deferrals

- **Localnet only.** On startup the bot reads the cluster's genesis hash
  and refuses to run against mainnet-beta, devnet, or testnet. Its
  airdrop needs the localnet faucet and its leader key holds no authority
  on a public cluster, so an off-localnet `--rpc` is always a
  misconfiguration — the guard fails fast rather than emitting doomed
  sends. The check is keyed on the genesis hash, not the RPC host, so a
  localnet on any address still passes while a port-forward to a public
  cluster is caught.
- **One supervisor, one leader.** This localnet plumbing runs all
  markets from one process under one quote-authority. The delegated
  per-market `quote_authority` model (one hot key per market) is the
  devnet/mainnet promotion's concern.
- **`FreezeVault` is admin-only.** The bot signs only as the leader, so
  the hard kill-switch triggers (peg breach, TVL floor, critical
  imbalance) **halt quoting** (zero the profile, let levels expire) and
  alert for human review rather than calling the irreversible,
  admin-gated `FreezeVault` autonomously.
- **Fill detection** subscribes to the program's `emit_cpi!`
  `FillEvent`s (production-fidelity path): a dedicated thread runs a
  `logsSubscribe` and reads the events out of each transaction's inner
  instructions via `getTransaction`. One subscription covers every
  market the leader quotes; the supervisor routes each fill to its
  market by `event.market`. The per-market vault read reconciles that
  belief (catching a missed fill or external flow) and is the sole
  signal in the fallback path when no subscription is attached.
