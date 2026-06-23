# Localnet Docker stack

The seed of the Dropset localnet container stack. Today it runs one
service — a local **Solana Explorer** — and is the foundation future
localnet services (e.g. the market-making bot) join.

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

The first build clones and compiles the explorer from source and takes
a few minutes; later starts reuse the cached image and are instant.

Pin or bump the explorer version with the `DROPSET_EXPLORER_REF`
environment variable (a branch, tag, or full commit SHA); it defaults
to `master`.

Open an account against the localnet at (one line; wrapped here):

```txt
http://localhost:3000/address/<PUBKEY>
    ?cluster=custom&customUrl=http://127.0.0.1:8899
```
