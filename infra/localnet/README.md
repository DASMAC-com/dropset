# Localnet Docker stack

The Dropset localnet container stack. It runs the off-chain services of
the localnet MVP, each as a compose service, against a **host-run**
`solana-test-validator` (deliberately not containerized — the
`dropset-tui` control plane owns its lifecycle):

- a local **Solana Explorer** (`make explorer`),
- the **event indexer** — Postgres + worker + `/v1` API
  (`make indexer-up`, see [`docs/indexer.md`](../../docs/indexer.md) §8),
- the **maker + taker bots** (`make bots-up`).

The container services reach the host validator over
`host.docker.internal:8899` (RPC) / `:8900` (PubSub).

## Why a local explorer

The hosted `explorer.solana.com` is served from a **public** HTTPS
origin, and modern browsers block a public page from reaching a
**loopback** address:

- **Brave** blocks any public site from accessing `localhost` by
  default (its Localhost Resource Permission feature).
- **Safari** treats `http://localhost` from an HTTPS page as mixed
  content and blocks it.
- **Chromium** browsers enforce Private Network Access: a public page
  reaching a private/loopback address needs the server to return
  `Access-Control-Allow-Private-Network: true`, which
  `solana-test-validator` does not send.

So the hosted explorer stalls on "loading" against the localnet — not
a CORS or indexer problem (the validator's CORS is fine), purely the
public-origin → loopback block. Serving the explorer from
`http://localhost` makes the page itself loopback, so its client-side
RPC fetch to the loopback validator is loopback → loopback and no
browser blocks it.

## Usage

The `dropset-tui` control plane owns the stack's lifecycle: it builds
and starts the explorer in the background at launch (so it is serving
by the time you open it) and tears it down on quit. Drive it by hand
with:

```sh
make explorer       # build (first run) + start, detached
make explorer-down  # stop and remove
```

The explorer is published on host port **3100** (the container serves
`3000` internally). That leaves `localhost:3000` to the frontend's
`next dev` (`make frontend`), so the explorer and the frontend can run
at the same time.

The first build clones and compiles the explorer from source and takes
a few minutes; later starts reuse the cached image and are instant.

Pin or bump the explorer version with the `DROPSET_EXPLORER_REF`
environment variable (a branch, tag, or full commit SHA); it defaults
to `master`.

Open an account against the localnet at (one line; wrapped here):

```txt
http://localhost:3100/address/<PUBKEY>
    ?cluster=custom&customUrl=http://127.0.0.1:8899
```

## The bots

The maker and taker bots run from a single shared image
(`bots.Dockerfile`, both binaries) with one compose service each:

```sh
make bots-up    # build (first run) + start maker-bot + taker-bot
make bots-down  # stop and remove both
```

Prerequisites: a host-run validator with the program deployed and the
mock CADC/USDC market bootstrapped and seeded — bring it to `Ready`
with the `dropset-tui` control plane first. The bots read the cluster's
genesis hash on startup and **refuse any non-localnet cluster**, so a
misconfigured RPC fails fast rather than signing against a public chain.

Both sign with the repo `keys/` keypairs, bind-mounted read-only:

- the **maker** quotes as the leader (`keys/EEEE.json`) and funds it
  from the localnet faucet over RPC — no host wallet needed. Set
  `CMC_API_KEY` in the environment to arm the CoinMarketCap secondary
  tier; without it the feed cascades CoinGecko → FX-rate → static.
- the **taker** signs swaps with `keys/FFFF.json` and mints itself back
  up to target inventory under the mock-mint authority
  (`keys/BBBB.json`, the committed localnet admin the host TUI created
  the mints under) — no host wallet needed.

The first build compiles the bots from source (slow); later runs reuse
the cargo-chef dependency cache.
