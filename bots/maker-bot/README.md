<!-- cspell:word aerodrome -->

<!-- cspell:word coingecko -->

<!-- cspell:word oanda -->

# dropset-maker-bot

The localnet market-maker for the mock CADC/USDC market. A single
leader bot that quotes CADC against USDC on the eCLOB per
[`docs/market-making-mvp.md`](../../docs/market-making-mvp.md): it polls
external price feeds, composes a fair mid from the CADC market-price
sources (with the Oanda FX feed as a peg sanity bound), and drives the
program's relative-quoting hot path (`set_reference_price`, with an
inventory skew) and cold path (`set_liquidity_profile`) under the spec's
inventory / peg / staleness kill switches.

## Layout

The crate follows the dropset-alpha maker-bot split:

- `config` — the spec's knobs, with defaults encoding the MVP verbatim
  (feed sources and cadences, the 50/100/200/500 bps ladder, the
  reference / profile triggers, the linear inventory skew, the
  kill-switch bounds).
- `model` — the pure, unit-tested quoting logic: feed composition
  (`fair_mid`), the ladder builder, inventory valuation and skew, the
  update-cadence triggers, and the kill-switch policy.
- `context` / `chain` / `tasks` — runtime state, on-chain I/O (market
  discovery, vault reads, the two quoting-path sends), and the
  5-second tick loop.

## Running

Prerequisites: a localnet `solana-test-validator` with the program
deployed and the mock CADC/USDC market bootstrapped and seeded (the
`dropset-tui` control plane does this — bring the market to `Ready`).

Dry run — poll the feeds once and print the reference and ladder the bot
*would* stamp, with no validator and no writes (the wiring check for
feed credentials):

```sh
cargo run -p dropset-maker-bot -- --dry-run
```

Live — discover the market, fund the leader from the faucet, and drive
the tick loop:

```sh
cargo run -p dropset-maker-bot
```

### Flags

- `--rpc <url>` — RPC endpoint (default `http://127.0.0.1:8899`).
- `--leader-key <path>` — leader / quote-authority keypair (default
  `keys/EEEE.json`, the role key the bootstrap seeds the vault with).
- `--aerodrome <network>:<pool>` — enable the Aerodrome (GeckoTerminal)
  CADC/USDC feed, off by default pending live verification of the pool
  and its base/quote orientation.
- `--dry-run` — poll feeds and print the intended quote, then exit.

### Environment

- `OANDA_API_KEY` — Oanda Practice API key for the FX peg sanity feed.
  Without it the peg kill switch is disarmed (Oanda staleness is
  non-fatal); the CADC sources still drive quoting.

## Notes and deferrals

- **Single bot.** The MVP ships exactly the spec's single maker; the
  multiple-strategy-variant structure is deferred.
- **`FreezeVault` is admin-only.** The bot signs only as the leader, so
  the hard kill-switch triggers (peg breach, TVL floor, critical
  imbalance) **halt quoting** (zero the profile, let levels expire) and
  alert for human review rather than calling the irreversible,
  admin-gated `FreezeVault` autonomously.
- **Fill detection** rides the per-tick vault read (the reference's
  price-time nonce bumps on every flush). The `emit_cpi` event
  subscription is the production-fidelity path and is deferred along
  with the adversarial taker that would exercise it.
