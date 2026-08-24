<!-- cspell:word backfilling -->

<!-- cspell:word backpressure -->

<!-- cspell:word CETES -->

<!-- cspell:word CLMM -->

<!-- cspell:word EUROC -->

<!-- cspell:word exchangerate -->

<!-- cspell:word Robinhood -->

<!-- cspell:word Stooq -->

<!-- cspell:word XRPL -->

<!-- cspell:word CoinGecko -->

<!-- cspell:word stationarity -->

<!-- cspell:word TIMESTAMPTZ -->

# Dropset Data Feeds — Ingestion Framework and Market-Data Collection

Two things over one substrate. The **`feeds`** crate is a shared
framework for pulling external and on-chain data from a **source**,
decoding it into typed records, and fanning those records out to one or
more **sinks** — a durable Postgres store *and* live in-process
consumers such as the market-making bots. **Market-data collection** is
the standing capability built on it: ongoing, currency-agnostic capture
of external price history into the shared database, plus the analytics
computed over that history to calibrate maker logic. The same source
serves both — a collector persists it for later analysis while a bot
reads the live tail to quote against.

**Doc boundary.** Sections 1–6 are the framework: infrastructure shared
by the off-chain event indexer ([`indexer.md`](indexer.md)), the
market-data collectors, and the maker / taker bots. They define *how*
data is sourced and delivered, never *what* a consumer does with it —
the indexer's `/v1` and the bots' quoting logic
([`market-making.md`](market-making.md)) stay with the
consumers. Sections 7–12 are market-data collection: what is collected,
where it lands, and what is computed over it. Section 13 carries the
open questions of both.

**Terminology.** The component is **market-data collection** — the
`market-data/` crate, collector binaries plus analytics. The store is
**the shared `dropset` database**; price history is a set of tables
inside it, not a separate warehouse.

**Status.** The framework crate has landed under `feeds/` — the
`Source` / `Sink` traits, the runner, the store and forward sinks, the
framework-owned cursor store, and the feature-gated HTTP / RPC /
streaming adapters. Both sink paths have their first consumer: the
Coinbase candle collector on the store sink, and the maker bot's price
and fill transports on the live sink — the latter carrying the first
streaming source, an RPC `logsSubscribe` socket bridged through the
stream seam (§4). The venue adapters have been relocated into
`feeds/src/venues/` (§4): there is now one implementation of each venue —
Pyth Hermes, Coinbase (candles *and* spot ticker), Kraken, CoinGecko,
CoinMarketCap, ECB/Frankfurter — available to both sink paths, rather
than one per app. The collector polls Coinbase candles and the maker
polls the rest today; what changed is that either could use any of them.
The FX-anchor and basis **primaries** (§9) are now among them, so the
maker no longer quotes off the fallback tier alone. The eCLOB indexer has
since migrated onto the framework
too (§6), so every ingestion path in the repo now runs on one drive
loop. The collector crate now lives at `market-data/`.

______________________________________________________________________

## 1. Goals and non-goals

**Goal — one ingestion substrate, many consumers.** Adding a data
source should be: implement a `Source`, pick the sinks, ship it.
Everything else — the drive loop, backoff, cursor persistence, graceful
shutdown, the fan-out — comes from the framework, so neither a new
collector feed nor a bot's price feed re-solves it. Crucially, the
*same* source can drive a durable store and a live consumer at once.

**In scope:**

- A **`Source`**: fetches or subscribes to a data source and yields
  typed records. Two drive shapes — a **poll** source (REST / RPC,
  backfill + interval) and a **subscribe** source (streaming / WebSocket
  push, for bot-latency consumers).
- **`Sink`s** that consume records, fanned out per source:
  - a **store sink** — idempotent Postgres persistence behind a
    resumable cursor (the durable path: market-data collectors, the
    indexer);
  - a **live sink** — an in-process channel that forwards records to a
    subscriber with minimal latency and no persistence (the bot path).
- A **runner** that drives a source and fans each batch to its sinks:
  tight loop while backfilling, interval once caught up, backoff on
  error, clean exit on `ctrl-c` / `SIGTERM`.
- A framework-owned **cursor store** (`feed_cursors`, JSONB per source),
  owned by the store sink; a live-only consumer needs no cursor.
- **Source adapters:** HTTP-REST, RPC-poll, and a streaming/WebSocket
  adapter — each feature-gated.

**Non-goals:**

- **The consumers' own logic** — the indexer's aggregation and `/v1`,
  and above all the **bots' quoting / trading decisions**. The framework
  delivers records; a bot decides what to do with them.
- **The store destination beyond Postgres.** The store sink targets a
  `PgPool` and the connection string decides local vs. cloud; the
  staged promotion to Aurora and the S3 archival tier are §12, not the
  framework's concern.

______________________________________________________________________

## 2. What already exists — the reuse surface

The framework is an **extraction and unification**, not a green-field
invention. Two consumers already ingest data their own way; the
framework lifts the common shape out of them:

- **Indexer — durable RPC poll.** The indexer's own `RpcPollSource`
  polled `getSignaturesForAddress` + `getTransaction` at `finalized` and
  returned base58-decoded inner-instruction blobs; `Store`
  (`indexer/src/store.rs`) is the `sqlx` pool + idempotent `ON CONFLICT`
  writers (it ran its own `sqlx::migrate!` until §8's single schema owner
  took that over); `Cursor` is a typed watermark. Its own comment named
  the seam: *"the geyser path would implement the same `poll` shape
  behind the same decode + store seam."* That is the poll source + store
  sink — and the migration in §6 has since landed, so the transport now
  lives in `feeds/src/rpc.rs` and the indexer keeps only its event
  decode, the row writers, the aggregator, and `/v1`.
- **Maker bot — a live price source and a live fill source.** The
  maker bot already composes a price feed (a CoinGecko → FX-rate →
  static cascade) to build a fair mid, and subscribes to fills via a
  blocking `logsSubscribe` → `getTransaction` inner-instruction walk to
  drive its position. Both are *live sinks* in this framework's terms —
  a source feeding an in-process consumer with no persistence — and are
  the precedent for the live path.

So the framework does not invent the poll loop or the live tail — it
generalizes the indexer's poll-into-store and the bot's
subscribe-into-memory into one source/sink model, then each consumer
migrates onto it (separate tasks) so none re-derives the seam.

______________________________________________________________________

## 3. The abstraction

```text
                          ┌─▶ store sink  (Postgres: idempotent
 source ─▶ [records] ─────┤                upsert + cursor advance)
 (poll or subscribe)      └─▶ live sink   (in-process channel ─▶ a
                                           bot reads the tail)
        ▲
        │  runner: drive the source; fan each batch to every sink;
        └─ sleep when caught up, back off on error, exit on shutdown
```

**`Source`** — where records come from:

- `fn name(&self) -> &str` — a stable identifier (cursor key, logs,
  metrics), e.g. `cex:coinbase:EURC-USDC`.
- `async fn next(&mut self) -> Result<Batch>` — fetch/receive the next
  batch of typed records and report whether it is caught up to the
  present. A poll source computes its window from the store sink's
  cursor (for backfill); a subscribe source blocks on the stream.

**`Sink`** — where records go. A source is wired to one or more:

- **Store sink** — persists a batch inside one transaction with
  `ON CONFLICT DO NOTHING`, then advances and saves the cursor. This
  owns the resumable position; it is what makes a feed backfill and
  restart safely.
- **Live sink** — pushes the batch onto an in-process channel (a
  `tokio` broadcast / mpsc). A bot subscribes and reads the tail; there
  is no cursor and no persistence — latency, not durability, is the
  point.

A feed = a source + its sink set. A **collector** wires its sources to a
store sink; a **bot** wires a price source to a live sink; a source that
serves both (persist *and* quote off the same data) is wired to both.

**`run`** — the runner: drive the source, fan each batch to every sink,
sleep `poll_interval` when caught up, retry after `error_backoff` on
error, and stop on `ctrl-c` / `SIGTERM`.

**`CursorStore`** — framework-owned, used by the store sink:

```text
feed_cursors (feed TEXT PRIMARY KEY, cursor JSONB NOT NULL,
              updated_at TIMESTAMPTZ NOT NULL DEFAULT now())
```

Each source serializes its own opaque cursor shape (a CEX feed stores
`{ "next_start": <epoch> }`, an RPC feed a signature or slot), so the
framework never knows the shape.

The framework *defines* this table but does not *create* it: like every
other table, it is created by the migration runner of §8. `PgCursorStore`
reads and upserts rows and nothing more.

**Delivery semantics — at-least-once (store sink).** The cursor is saved
*after* the batch commits. A crash between commit and cursor-save
re-fetches the last window on restart, and the idempotent upsert absorbs
the duplicate. The live sink is best-effort by design — a slow bot
consumer drops to the latest rather than stalling the source (§13).

______________________________________________________________________

## 4. Adapters

Two layers, and it is worth keeping them apart: **transports** are the
reusable connectors a source composes, each **feature-gated** so a
consumer compiles only the one it uses; **venue adapters** are the
concrete sources built on a transport, one per venue.

Both **belong in `feeds/`**, so a venue is written **once** and consumed
by collectors and bots alike rather than stranded in whichever app
needed it first. The venue adapters live under `feeds/src/venues/`,
one module per venue, and they know nothing about sinks — a collector
wires one to a store sink and keeps the history, the maker wires the
same source to a forward sink and quotes off it.

### Transports

- **HTTP-REST** (`feature = "http"`, `reqwest` over TLS) — a small JSON
  client: a base URL, a shared client, and `get_json(path, query)`.
  Consumers: the Coinbase reference feed, the FX and issuer-rate feeds,
  and the maker's own price polls.
- **RPC-poll** (`feature = "rpc"`, the solana 3.x client tree) —
  `RpcPollSource`, generalized over program id from the indexer's
  original poll source: enumerate signatures backwards to the resume
  cursor, then emit oldest-first, fetching each transaction at
  `finalized` and flattening inner instructions into ordered, decoded
  blobs. It is generic over an `RpcTransport` seam — the shape a geyser
  transport would implement. Consumer: the
  eCLOB indexer.
- **Streaming / WebSocket** (`feature = "stream"`) — a subscribe source
  for the low-latency bot path (a CEX ticker socket, an RPC
  `logsSubscribe`, or geyser). The first concrete adapter is the maker's
  fill socket; a CEX price socket follows when a basis leg needs
  lower latency than its poll cadence gives.

Features are **off by default** so an HTTP-only consumer never compiles
the Solana or streaming trees.

### Venue adapters

The venue's endpoint, not taste, decides an adapter's shape:

- **Batched quote venues** follow a stated batched-poll convention —
  built with a whole symbol set, one inherent `poll` per source
  returning a `symbol → price` map. CoinGecko, CoinMarketCap,
  ECB/Frankfurter, and Kraken are the four today. This is the per-venue
  budget's main lever (§10): one poll for N markets rather than N polls.
  A symbol the venue does not quote is omitted, never an error — one
  unlisted token must not dark the rest of the roster. That `poll` stays
  **public** alongside the source's `Source` impl, whose `next` wraps the
  same poll in a batch, so one adapter drives the runner *and* answers a
  caller wanting a single synchronous reading — a `--dry-run` credentials
  check — with no runner at all.

  The convention is held by review rather than by a trait, on purpose.
  Each venue keys symbols its own way (CoinGecko slugs are strings,
  CoinMarketCap listing ids are numeric), so no single collection could
  ever hold these adapters polymorphically, and no caller consumes them
  that way — the polymorphic seam for ingestion is `Source` / `Sink`,
  one layer up. A future uniform poller that wants a common consumer
  designs that abstraction against its own needs.

- **Per-product history venues** get one source per product, because
  batching is not on offer. Coinbase's candles endpoint is the case; it
  pages its own backfill instead and emits only closed buckets.

**Credentials are injected, never read inside an adapter.** A keyed
adapter takes its key as a constructor argument, so *where the secret
comes from* stays a deployment decision the consuming app owns. That
decision is now the secrets provider (§12): the collectors resolve
through it. It is not visible to the adapter — the only thing an adapter
declares is its credential's canonical name, next to the constructor that
wants it. The maker resolves nothing at all, because no venue in its
cascade is keyed.

**Prefer a venue's keyless route where one exists, and not only for
convenience.** A keyed free tier tends to price access as a *quota* —
credits per month — while a keyless public route prices it as a *rate*,
answered with a 429. Rate is a thing this crate's shared client can hold
(§10); a monthly quota is not, because a minimum interval is in-process
state that resets on restart. So the keyless route removes a class of
exposure rather than merely relocating it, and it keeps the maker's whole
feed cascade secret-free. CoinMarketCap is the worked example: its keyed
`Basic` plan caps out at 15,000 credits/month and its plan table bills
that tier for personal use, where its keyless
`/public-api/v1/simple/price` publishes no monthly allowance at all.

Two caveats on that example, because it is the argument for the whole
preference. The keyless route still reports a `credit_count` per
response, so it is metered even though no allowance is published — the
claim is "no published quota", not "no accounting". And only the keyed
plan's personal-use billing was checked; the keyless route's licensing
was **not** confirmed, and provider terms commonly bind all access
however it is authenticated. Prefer the keyless route for the budget
reason, which is verified; do not lean on it as a licensing finding.

**A credential rides the transport as a sensitive header.** An adapter
that authenticates with a header passes it to
`HttpClient::with_secret_header`, never plain `with_header`: the value is
marked sensitive, which keeps it out of any `Debug` render of the header
map and out of HTTP/2's HPACK dynamic table. This is a constructor-level
guarantee on purpose — whether a key can leak must not depend on which
types happen not to derive `Debug` yet. Plain `with_header` remains the
right call for a benign header, such as OANDA's UNIX datetime-format
preference, where debug visibility is worth keeping.

**A URL-borne credential goes through its own constructor.** Alpha
Vantage and Twelve Data authenticate with an `apikey` **query
parameter**, so no header marking reaches them, and the sink is a
different one: a `reqwest` error carries the *effective* URL — query
string included — and renders it in its own `Display`, so the key
surfaces in any `{:?}` of the resulting `anyhow` chain, which is exactly
what a top-level handler logs. It needs no hostile venue, only an
ordinary request failure. Such an adapter passes its key to
`HttpClient::with_secret_query_param`, which appends it to every request
and redacts its value out of every transport error before it is wrapped.
Carrying the key on the client rather than in the adapter's per-request
query is the point: the transport then knows which parameter is a
credential. Passing a key through `get_json`'s `query` instead bypasses
the mechanism, exactly as plain `with_header` would. Redaction is
targeted, not blanket — benign parameters stay legible, because a failed
paged backfill is diagnosed from precisely those.

**Redirects are refused, which is the third credential boundary.**
`reqwest`'s default policy follows up to 10 redirects and strips
credentials across a cross-host hop *by header name* only
(`Authorization`, `Cookie`, `cookie2`, `Proxy-Authorization`,
`WWW-Authenticate`), never consulting the sensitive marking. A
custom-named key header is not on that list, so it would be replayed
verbatim to whatever host a redirect named — wire-to-a-third-party, the
one sink a sensitive flag cannot cover. So `HttpClient::new` sets
`Policy::none()`: every venue polled here is a canonical JSON API host
answering directly (probed — all eight answer without a 3xx), and a 3xx
is surfaced as an explicit error rather than followed. Refusing outright
fails loudly if a venue ever starts redirecting, which beats a silent
key disclosure.

**That boundary is preventive, and it matters that it is.** Every keyed
adapter today is safe by accident of naming: OANDA's bearer rides
`Authorization`, which is stripped, and the two query-parameter venues
touch no header at all. The header that motivated the boundary — a
custom-named venue key — was retired when CoinMarketCap moved to its
keyless route, so the exposure is currently unrealized. It re-opens
silently the first time a venue authenticates by a custom header, which
is exactly how the retired one worked. Pinning the policy makes the
guarantee a property of the transport rather than of the current roster,
which is the only form of it that survives a roster change.

______________________________________________________________________

## 5. Process and deployment model

Two deployment shapes, because the two sink kinds live in different
processes:

- **Store-sink feeds run as their own processes / containers.**
  Separate binaries per feed; one versioned Docker image builds a family
  of them, and the compose `command` selects the process — the same
  mechanism locally and on the deployed host (§12). The migration runner
  is its own image, because the schema owner is a separate deploy unit
  that runs to completion before any consumer starts and must not be
  versioned with one. A run-once migration task precedes one long-lived
  service per feed against the same database; every dependent service
  gates on it exiting successfully. Every feed is idempotent and
  cursor-resumable, so a restarted task just resumes.
- **Live-sink feeds run in the consumer's process.** A bot links
  `feeds`, constructs a source, wires it to a live sink, and reads the
  channel — no separate container, no database. The same adapter code
  serves both shapes; only the sink and the host process differ.

**What a host ingests is decided by deployment composition** — which
processes the compose file runs — **not by code flags.** The same
images run locally and in the cloud; the compose file is the difference.

**One service per venue, not per pair.** A store-sink collector takes a
**roster** of canonical product ids and covers all of them, so widening
coverage is a configuration change rather than another container. The
alternative — which this replaced — was one service per pair, so N pairs
cost N processes, N connection pools, and N units to schedule to do what
is often a single batched request.

How a roster becomes work depends on the venue's endpoint, and both
shapes live behind the same configuration:

- **Batched venues** (Kraken's ticker, Pyth's latest-price) price the
  whole roster in one request, so the process runs a single feed whose
  records fan out to many products. Adding a pair costs no extra request
  at all.
- **Per-product venues** (every candles endpoint, Coinbase's ticker) need
  one source per product, so the process supervises several feeds
  concurrently. They **share one HTTP client**, which is a rate-limit
  requirement rather than a saving: clones of a client share its pacing
  budget while a second client opens an independent one, and every feed
  here reaches the same host from the same egress IP — which is what a
  keyless tier limits on (§10). A client per product would multiply the
  venue's budget by the roster size.

Two properties keep the split safe. **Cursor keys are per product**
(`fx:oanda:AUD-USD`), never per process, so consolidating N per-pair
services into one per-venue service resumes every cursor exactly where it
was. And **a failed feed fails the process**: a collector still running
with two of its five pairs dead looks healthy to everything watching it,
while the store's coverage is what actually gets read — so crashing and
resuming from committed cursors is the honest behavior.

______________________________________________________________________

## 6. Consumers and boundaries

- **Market-data collectors (store sink).** The collectors build their
  sources on this crate into a store sink: an HTTP Coinbase reference
  feed first (the proof feed), then the FX and issuer-rate feeds. See
  §7 onward. (An econ-calendar feed was planned here and is deferred:
  see §13's resolved "Econ-calendar source" entry, and
  `docs/market-calendar.md`.)
- **Maker bot (live sink, landed).** The maker-bot's tiered price legs and
  `logsSubscribe` fill walk run on `feeds`: the HTTP price sources — Pyth
  Hermes and ECB/Frankfurter for the FX anchor, Coinbase and Kraken for
  the basis and the USDC peg, CoinGecko / CoinMarketCap behind them — and
  the fill `logsSubscribe`
  socket — bridged through the stream seam (§4) — fan onto in-process
  forward (live) sinks its synchronous tick loop drains with `try_recv`, on
  a small background runtime. Coinbase's ticker is keyed by one product,
  so that tier is one source per listed market rather than one batched
  poll. The taker bot has no bespoke price or fill
  feed to migrate: it is a stochastic flow generator sizing orders against
  the live on-chain book, so it stays as the *producer* of the fills the
  maker now consumes.
- **The eCLOB indexer (store sink, landed).** The indexer runs on the
  RPC source + store sink + cursor and keeps its own writers, aggregator,
  and `/v1`. Its writers became a `StoreWriter` (decode-and-write inside
  the framework's batch transaction) and its aggregator a second `Sink`
  ordered after the store sink, so one batch's ingest and fold stay in
  step exactly as the hand-rolled worker had them. The migration also
  made ingest **resumable**: the old loop always started from the
  present, so a restart skipped whatever landed while it was down, and
  the framework's `feed_cursors` position closes that. It brought no DDL
  — the cursor table is §8's carve-out and already exists.

______________________________________________________________________

## 7. Market-data collection — a standing capability

Collection is **ongoing and currency-agnostic**. There is no gate, no
go/no-go decision it terminates at, and no one-shot milestone sequence:
price history is a standing asset that gets more valuable the longer it
runs, and a history not collected today cannot be backfilled from a
venue that only serves a rolling window.

What it is for:

- **Calibrating the maker's slow variables.** The basis EMA half-life,
  the realized-σ inputs to the quote ladder, and the per-market sane
  bands are *estimated from history*, not guessed
  ([`market-making.md`](market-making.md) §1 and §4 both defer
  to this doc for exactly those numbers — see §11).
- **Currency-entry evidence.** Before quoting a new FX stablecoin, its
  own history has to say whether its basis reverts at all. Reversion is
  asserted per market, never assumed for the roster.
- **Analytics and data science on maker logic** — regime studies, and
  the durable substrate any later modeling work needs.

**Collection never sits in the quoting path.** Quoting depends on live
feeds and nothing else. The database holds the full price history (§8),
but what the **quoting path** may read out of it is **slow variables
only** — parameters, volatility estimates, regime tags — never a price
it is about to quote against. A collector outage, or
the database being unreachable, degrades tomorrow's calibration — it
must never stop today's quotes. This is a hard invariant, not a
preference, and it is why the maker keeps its own deliberately
independent polls rather than reading prices back out of Postgres.

______________________________________________________________________

## 8. The shared `dropset` database

**One Postgres instance, named `dropset`**, holds everything: indexer
tables (on-chain activity), market-data tables (external prices),
maker go-between tables (parameters, telemetry), and the derived
analytics. One instance rather than one per app, because the joins that
matter cross the boundary — attributing realized PnL means joining
indexer fills against the external price history at fill time, and that
should be a query, not an export.

**Single schema owner.** Only the migration runner issues DDL, and
migrations are versioned and additive-only. No app creates its own
tables at startup; a collector that finds its table missing fails loudly
rather than conjuring one, so the schema has exactly one source of
truth.

That owner is the **`db-schema/`** crate (`dropset-db-schema`): one
ordered migration directory, and the **`dropset-migrate`** binary that
applies it. Three regimes preceded it — the framework migrated
`feed_cursors`, the indexer ran a second `sqlx::migrate!` from inside
`Store::connect`, and the collector applied idempotent
`CREATE TABLE IF NOT EXISTS` startup DDL precisely to avoid colliding
with that second migrator on the shared `_sqlx_migrations` table. They
coexisted only because each pointed at a different database; one instance
is what forced the consolidation.

Two properties are load-bearing:

- **Additive-only, enforced by a `>=` fence.** DB-primary apps (the
  indexer, the collectors) call `require_schema` at startup, which
  asserts the database's applied history *covers* the version the binary
  was compiled against. The comparison is `>=`, never `==`: during a
  deploy an old binary may briefly run against a newer schema, and that
  window is supported by design. Only a database *behind* the binary is
  an error, and it names the fix.
- **The fence is for DB-primary apps only.** The maker's quote path and
  the TUI go-between keep the opposite contract: Postgres there is a soft
  dependency, so a database that is unreachable, or behind on its
  migrations, means degraded operation surfaced in telemetry, never a
  refusal to start. Wiring the fence into them would convert a tolerated
  condition into an outage.

The runner doubles as the local reset story — point it at a fresh
database and the full history replays — and, because it is idempotent, as
a compose init step that re-runs harmlessly on every restart.

**Table ownership — one writer, unrestricted readers.** Every table has
exactly one writer app. Reads are deliberately unrestricted; that is the
point of sharing an instance.

Unrestricted, but not anonymous: `dropset_ro` is a **read-only login**
(`db-schema/migrations/0002_reader_role.sql`) holding `SELECT` on every
table — plus `CONNECT` and schema `USAGE`, and no write privilege of any
kind. Tables a later migration adds are covered too, by
`ALTER DEFAULT PRIVILEGES`, though only while migrations keep being
applied by the same role that ran `0002`; one run as a different role
would create tables the reader cannot see, which surfaces as an empty
panel. A
consumer that only reads connects as that role rather than as the
`dropset` owner, which turns the one-writer rule from an honor system
into a privilege — a dashboard cannot write a table by accident. Grafana
(`market-data/grafana/`) is the first such consumer. It is one shared
reader rather than a role per consumer on purpose: every reader needs
exactly the same grants, so splitting them would multiply bookkeeping
without buying isolation.

| Table                                                              | Writer             | Contents                                                    |
| ------------------------------------------------------------------ | ------------------ | ----------------------------------------------------------- |
| `feed_cursors`                                                     | `feeds` store sink | Resumable per-feed position (JSONB)                         |
| `cex_prices`                                                       | market-data        | CEX reference candles, per venue and product                |
| `spot_ticks`                                                       | market-data        | Point-in-time prints, per venue and product                 |
| `pyth_fx_feeds`                                                    | migration (seed)   | Pyth FX roster — venue reference data, read-only at runtime |
| `fx_rates` *(planned)*                                             | market-data        | Fiat-cross bars for the FX anchor leg                       |
| `peg_rates` *(planned)*                                            | market-data        | Issuer / redemption reference rates                         |
| `fx_events` *(deferred)*                                           | market-data        | Economic-calendar event times — see §13                     |
| `fx_sessions` *(planned)*                                          | market-data        | Generated FX session / week instants (UTC)                  |
| `basis_series` *(planned)*                                         | market-data        | Derived per-market basis series                             |
| `vol_estimates` *(planned)*                                        | market-data        | Realized volatility by market and window                    |
| `regimes` *(planned)*                                              | market-data        | Regime tags every other stat is sliced by                   |
| `fill_events`, `events`, `takes`, `market_stats`, `indexer_cursor` | indexer            | On-chain event capture and its rollups                      |
| Maker parameter and telemetry tables *(planned)*                   | maker go-between   | Slow parameters published to the bot; run telemetry         |

Adding a table means naming its writer here. A table with two writers is
a design error, not a configuration choice.

The one carve-out is `feed_cursors`, which the **framework** owns rather
than any single app: every store-sink process writes its own row, keyed
by the feed name. Writers partition by key, so the rule holds at the row
level even though several apps touch the table — and the framework, not
an app, defines its shape. A table wanting that treatment has to earn it
the same way: a key that makes the partition structural, not a
convention two writers agree to keep.

**Two row shapes for prices, and the distinction is deliberate.**
`cex_prices` holds candles — aggregates over a window, keyed by that
window's start. `spot_ticks` holds ticks — single observations, with no
window at all. They are separate tables rather than one table at two
granularities because a tick stored as a one-second "candle" would put a
fabricated bucket width in the key, make `open`/`high`/`low`/`close` four
copies of one number, leave no honest place for a confidence half-width,
and silently corrupt every query that reads `cex_prices` as bars. The
tick tier exists because the finest bucket any candle endpoint offers is
60s, so no polling cadence makes a candle series show movement *between*
closes.

**Reference data is a third kind of table, and `pyth_fx_feeds` is the
first of it.** It is not a feed's output; it is configuration a collector
*reads*. Its writer is the migration that seeds it — there is no runtime
writer at all — so the one-writer rule holds trivially, and a reader that
found itself wanting to write one would be reaching for the parameter
channel instead, which is a different design with different semantics
(desired vs. applied state, a TUI writer, an audit trail). Keeping the two
apart matters: slowly-changing venue coordinates want a migration and a
restart, while operator-tunable runtime parameters want neither. See §12
for why this particular configuration lives in the database rather than in
the environment or a constant.

______________________________________________________________________

## 9. Sources and venue policy

Fair value is **`fx_rate × basis`** — a deep, exogenous FX anchor
corrected by a slow, thin stablecoin basis, decomposed across both pegs
(`basis = (token / fiat) ÷ (USDC / USD)`). The model itself, its
regimes, and its failure modes belong to
[`market-making.md`](market-making.md) §1; what follows is which
sources feed each leg and on what terms.

| Role                             | Source                                                                             |
| -------------------------------- | ---------------------------------------------------------------------------------- |
| FX anchor (`fiat/USD`)           | Pyth Hermes FX + the FX roster below, all wired; ECB / Frankfurter daily reference |
| Basis (`token/fiat`, `USDC/USD`) | Coinbase `<token>/USDC` (wired), Kraken `<token>/USD` (wired)                      |
| Peg truth                        | Kraken `USDC/USD` (wired); Circle / issuer redemption rate                         |
| Token/USD, thin coverage         | CoinGecko / CoinMarketCap — reflexive, and the only basis source most markets have |
| Macro overlay                    | *Deferred* — was an econ-calendar loader; see §13                                  |

**These are candidates, not a priority ladder.** Every source listed for a
leg is offered to the model together, and the leg is resolved by consensus
across whichever of them are healthy — see
[`market-making.md`](market-making.md) §1 "Leg resolution" for the rule.
Order still decides which sources fill a leg that has more than it can hold,
and nothing else. The practical consequence is where the wording below used
to say "fallback": an aggregator index is not a lower tier waiting to be
reached, it is a second opinion whenever it answers, and for the six markets
no CEX lists it is the only opinion there is — which the model now reports as
uncorroborated rather than treating as a price like any other.

Pyth is the one source designated believable **on its own**, because it
publishes a confidence half-width and is aged from the publisher's clock. A
lone reading from anything else is reported as uncorroborated.

### The session clock is not a leg source

The FX session and week instants (`docs/market-calendar.md`) are
deliberately **absent from the table above**. Every row there is a
price source offered to a leg's consensus, and a clock is not one: it
carries no value to corroborate, so it belongs to no leg and cannot be
a candidate in the sense the paragraph above means.

What it is instead is a **slow-variable clock context**, consumed
alongside the composed value rather than inside it. It answers two
questions: whether a leading FX feed is expected at all right now — so
a silent anchor reads as scheduled rather than broken — and which
sessions are overlapping, so the maker widens where volatility is
known to concentrate. Both act on the quoting posture, never on a leg's
price.

Two consequences worth stating here rather than leaving to the
calendar doc. It is **generated** from a committed rule table rather
than fetched, so it draws no venue budget (§10) and has no provider to
fall back from. And its failure mode is an **expiring horizon** rather
than an outage, so a consumer past the generated-through watermark must
treat the calendar as unavailable and fall back loudly — never infer
"closed" from a missing row.

### What is wired, and why the rest is not

The sources above are keyless and live, except where noted. The gaps are not
oversights — each was probed and ruled out on evidence:

- **Binance is unusable from the deploy region.** `api.binance.com` answers
  `HTTP 451` from both a developer machine and `us-west-2`, so the spec's
  `EUR/USDT` basis leg would be dead code in the deployment rather than
  merely untested. `api.binance.us` is reachable but lists **no EUR pair at
  all**, and its one relevant symbol, `USDCUSD`, prints an administered flat
  `1.00000000` — a feed that would report a depeg as perfect health. Kraken
  takes that slot instead.
- **Circle publishes no keyless redemption rate.** `/v1/exchange/rates` is
  credentialed (`401`); the only public endpoint returns circulating supply
  per chain, not a rate. Peg truth is therefore *observed at a venue* rather
  than read from the issuer: Kraken's `USDC/USD` is a real market print of
  the peg, and it is the leg that is **wired**. Kraken also lists `EURC/EUR`
  — the cross redemption arbitrage enforces directly, and the natural
  issuer-rate proxy — which the tick collector now **records**, though no
  model leg reads it: the maker's roster still asks only for `<token>/USD`
  plus the shared `USDC/USD`. It is collected ahead of its consumer because
  the poll is batched, so the pair costs no extra request, and the series
  cannot be reconstructed afterwards: Kraken's keyless OHLC serves only a
  rolling window (~12 hours of 1-minute bars, 30 days of hourly), so tick
  resolution exists only where something was already recording. A
  credentialed Circle Mint feed supersedes both when keys exist.
- **OANDA is credentialed but the credential is free**, which is why it is now
  wired rather than deferred: a practice account issues a v20 token at no cost
  and serves minute FX bars back **years**. Pyth Hermes remains the streaming
  anchor with its confidence half-width, which OANDA would have to be asked
  for separately; OANDA is the deep *history* the store needs and the
  independent second opinion the fusion estimator needs. The two are
  complements, not alternatives.

**Coverage is asymmetric, permanently.** Of the seven demo tokens only EURC
reaches a CEX (Coinbase `EURC-USDC`, Kraken `EURC/USD`). The other six trade
on neither, so their basis leg has no primary tier and the CoinGecko /
CoinMarketCap indices carry it — the fallbacks are load-bearing, not vestigial.

Two decode details Pyth forces, both handled in the adapter. It publishes each
cross **one way only**, and for five of the seven roster currencies that is
`USD/<ccy>`, so those are reciprocated — with the confidence half-width
transformed as `δ(1/p) ≈ δp / p²`, since a half-width does not survive
inversion unchanged. And its FX feeds follow the interbank schedule, so
readings are aged from the publisher's `publish_time` rather than from
receipt: a consumer ageing from receipt would see a frozen weekend rate as
perpetually fresh and never engage the crypto-only regime.

**Coinbase is the proof feed and the first adapter.** The Exchange
public REST API is keyless and reachable; its candles endpoint returns
`[time, low, high, open, close, volume]` arrays (epoch seconds, at most
300 buckets per request, epoch `start` / `end` accepted), which
backfills and polls cleanly — it validated the whole framework end to
end before any harder source.

### The tick tier — every wired venue, into the store

The adapters above served only the maker's live path at first: Pyth
Hermes, Kraken, and Coinbase's ticker fed in-process forward sinks and
nothing was recorded. Each now also has a **store** collector writing into
`spot_ticks` (§8), which is what fills the dashboard's multi-source overlay
and makes a dislocation readable after the fact rather than only watchable
live.

| Collector                     | Shape                      | What it uniquely carries                                        |
| ----------------------------- | -------------------------- | --------------------------------------------------------------- |
| `market-data-pyth`            | Batched, roster from store | A published **confidence** half-width and a publisher timestamp |
| `market-data-kraken`          | Batched, roster from env   | A real market print of `USDC/USD` — peg truth, wired            |
| `market-data-coinbase-ticker` | One feed per product       | The prints **between** candle closes on the reference venue     |

Three things about this tier are load-bearing.

**`observed_at` is the venue's publish time where the venue publishes
one**, else the poll second. Pyth does, so a re-polled reading carries the
same instant and lands on the primary key — the re-fetch a restart causes
is genuinely idempotent rather than a second row for one observation. The
others do not, so their attribution is the poll.

**A confidence of `NULL` means "no confidence notion", never zero.** Zero
would read as *perfect certainty* and silently satisfy a fresh-but-uncertain
gate that a missing value correctly fails, which is why the column
constrains itself to `NULL OR > 0` and the writer coerces a malformed
half-width to `NULL` rather than letting a `CHECK` violation crash-loop the
collector.

**A batched venue omits what it could not price, so a misconfiguration is
indistinguishable from an outage** — and that is the failure mode this tier
is most exposed to. A mistyped Pyth feed id (opaque hex, impossible to
catch by eye), a Kraken pair spelled the way *we* name it rather than Kraken
does, and a currency the venue simply does not publish all produce the same
thing: silence, with nothing logged. So the batched collectors track which
configured products have *ever* priced and, after a few polls, report the
ones that never did as a roster error rather than a venue gap. The
per-product Coinbase collector needs no such watch — each of its feeds is
named, so one that never yields is already identifiable in the logs.

The CEX WebSocket adapter remains **deliberately out of scope**: §13 holds
that question, and its trigger is a *quoting* need, not a visualization
one. Polled ticks at 15 s serve the dashboard.

### The free-tier FX roster

Three FX vendors are wired, each on a free credential (they live in the
local secrets enclave, §12 — the collectors name them
`oanda/api-key`, `twelvedata/api-key`, and `alphavantage/api-key`).
What each is *for* follows from what its free tier actually serves,
measured against a live key rather than read off a pricing page:

| Source         | Bars  | Free-tier budget        | History       | Volume     |
| -------------- | ----- | ----------------------- | ------------- | ---------- |
| `oanda`        | M1    | 100 req/s (a guideline) | 3+ years      | tick count |
| `twelvedata`   | 1min  | 800 credits/day, 8/min  | 60 d verified | **none**   |
| `alphavantage` | daily | **25 req/day**, account | to 2007       | **none**   |

- **OANDA is the anchor.** Deep minute history, a per-candle `complete`
  flag, 5000 candles per request, and a budget so loose the cadence is
  chosen for freshness rather than to dodge a limit.
- **Twelve Data is the cross-check.** It defaults to *exchange-local*
  time — a default AUD/USD request returned `10:26` when UTC was `00:26`,
  i.e. Sydney — so every request pins `timezone=UTC`. A ten-hour skew in
  `bucket_start` would look entirely plausible in the store.
- **Alpha Vantage is a daily corroboration only.** `FX_INTRADAY` is
  premium-gated (`"This is a premium endpoint."`), so no polling cadence
  buys a minute bar here. Its 25 requests/day is the whole account.
- **TraderMade was dropped**: no free key is obtainable. Its pricing page
  lists only paid plans and its signup is a sales form, while its own
  tutorials still advertise a free tier.

Two sources publish **no volume at all**, so their rows carry `0.0`.
`cex_prices.volume` is therefore comparable only *within* a source,
never across two.

**Weekend behavior differs by source, and the difference is the useful
part.** Real FX is closed from Friday evening to Sunday evening, and the
vendors disagree about what to publish then:

- **OANDA publishes nothing** — a weekend window returns zero candles.
- **Twelve Data publishes a complete minute grid**, all 1440 bars.

Observed live in `cex_prices` across one Friday close (21:00 UTC), both
collectors running against the same pair:

```text
source      | newest bar (UTC)    | bars after Fri 21:00
oanda       | 2026-08-14 20:59:00 |    0
twelvedata  | 2026-08-14 22:02:00 |   63
```

That run also exercised the thing a weekend breaks: OANDA's cursor
advanced past 21:00 to the present despite having no candle to show for
it. A source that anchored its cursor on the newest row returned would
park at every Friday close and never resume on Monday.

So **prefer the zero-bar source when the question is whether a session
existed**; bar *absence* is the cleanest signal available, and it is what
the crypto-only weekend regime should engage on. Never pool the two into
one volatility figure — they disagree about whether a market was open.

What this is **not** is evidence that either vendor is fabricating. A
measured Saturday range of 3.68 bps on Twelve Data's AUD/USD sits in the
same band as a genuinely traded 24/7 crypto tape's 6.92 bps
(EURC-USDC, same Saturday), and distinct-close counts do not separate
them either. It is a vendor coverage convention — Twelve Data returns a
gap-free grid on trading days too — and nothing more.

**The canonical symbol is ours, not the venue's.** These three spell one
pair three ways (`AUD_USD`, `AUD/USD`, and a split `from` / `to` pair),
so the stored `product_id` is a canonical hyphenated ISO-4217
`BASE-QUOTE` (`AUD-USD`) and each adapter is handed the spelling it wants
at construction. Storing venue-native symbols would land one pair under
three keys and make a cross-source comparison impossible. It is what
makes the join work: over 1608 overlapping minutes, `oanda` and
`twelvedata` agree to a mean absolute difference of **0.686 bps**
(r = 0.993), which is the end-to-end check that both decoders and both
timestamp paths are right.

**The third intraday source already exists, and it is Pyth.** Read as a
table of vendors, the roster above is two independent intraday sources
with a daily-only third behind them — the stated two-source floor with
no margin, and a two-input consensus can detect disagreement without
being able to adjudicate it. That framing counts one source short of
what is already wired. Pyth Hermes is keyless, intraday, publishes a
confidence half-width, and is the one source designated believable on
its own above. Where it is configured, the intraday composite already
has **three** inputs, not two. How its readings reach the store is a
separate question, answered by the tick tier above; what follows is
only which currencies it covers.

What is short is *configuration*, not vendors. Hermes publishes 290 FX
feeds; measured against the roster on 2026-08-24:

| Currency | Pyth symbol  | Live   | Confidence | Wired  |
| -------- | ------------ | ------ | ---------- | ------ |
| EUR      | `FX.EUR/USD` | yes    | 1.03 bps   | yes    |
| GBP      | `FX.GBP/USD` | yes    | 1.03 bps   | yes    |
| CHF      | `FX.USD/CHF` | yes    | 7.99 bps   | yes    |
| ZAR      | `FX.USD/ZAR` | yes    | 2.94 bps   | yes    |
| MXN      | `FX.USD/MXN` | yes    | 1.08 bps   | yes    |
| SGD      | `FX.USD/SGD` | yes    | 4.65 bps   | yes    |
| IDR      | `FX.USD/IDR` | yes    | 11.45 bps  | yes    |
| AUD      | `FX.AUD/USD` | yes    | 1.39 bps   | **no** |
| BRL      | `FX.USD/BRL` | yes    | 10.48 bps  | **no** |
| CAD      | `FX.USD/CAD` | yes    | 7.70 bps   | **no** |
| JPY      | `FX.USD/JPY` | yes    | 0.57 bps   | **no** |
| TRY      | `FX.USD/TRY` | yes    | 3.27 bps   | **no** |
| MYR      | `FX.USD/MYR` | **no** | —          | no     |
| NGN      | `FX.USD/NGN` | **no** | —          | no     |

**"Wired" means both places, because there are two and they agree.** The
maker reads its feeds from `bots/maker-bot/src/config.rs`; the collector
reads its roster from the `pyth_fx_feeds` reference table seeded by
migration `0005_pyth_fx_feeds.sql`. Those two carry the **same seven
currencies, the same feed ids, and the same invert flags** — so the
distinction between maker-wired and platform-seeded, which would matter
if they diverged, currently does not. Keep it in view anyway: they are
separate sources of truth updated by separate changes, and a future
widening that lands in one and not the other is exactly the divergence
this row would otherwise hide.

Two results carry. **A catalog entry is not a live feed**: MYR and NGN
are both published in the FX catalog and both return a price of exactly
zero at `publish_time` 0 — they have never published. Scoring coverage
off the catalog gives 14/14; measured against actual publish times it is
**12/14**, and the two that fail are the two thinnest currencies on the
roster. Anyone widening the roster from the catalog should read that as
a warning: the catalog will happily offer two feeds that never tick.
**Five live feeds are unwired** — AUD, BRL, CAD, JPY and TRY publish now
and are absent from both the maker config and the seed. Because the two
agree, wiring them is one coherent change rather than a reconciliation.

So: wire those five, and treat **MYR and NGN as the roster's most
exposed currencies**. They are the only two with no Pyth column at all,
so they depend on OANDA and Twelve Data alone — and note carefully that
this is an upper bound, not a measurement: per-currency OANDA and Twelve
Data coverage is verified only for AUD and EUR (§13), so MYR and NGN are
*at best* two-source and could be one or zero. That is precisely why
they are where a further source would buy the most.

**The roster for AUD/USD quoting, stated outright**, since it is the
pair this survey was opened on: **Pyth plus OANDA plus Twelve Data**,
with Frankfurter as the daily reference and Alpha Vantage as daily
corroboration. All three intraday sources are free, AUD is live on Pyth
at a 1.39 bps half-width, and the only outstanding work is wiring the
Pyth feed the maker config and the seed both lack.

**One caveat on independence, since the count invites more confidence
than it earns.** Three vendors is not three uncorrelated looks at the
market. Pyth's FX publishers are institutional feeds that may source the
same interbank prices OANDA and Twelve Data do, so the composite's
failure modes are correlated to a degree nothing here measures. Vendor
diversity bounds *outage* and *decode* risk, which is most of what has
actually gone wrong; it does not bound one bad interbank print
propagating to all three alike.

**Candidates probed and rejected**, recorded so the search is not
repeated:

| Source                | Intraday      | Verdict                                                                                                                                        |
| --------------------- | ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `exchangerate.host`   | —             | a key is now required; the free keyless tier is gone                                                                                           |
| `open.er-api.com`     | no            | reachable and keyless, but daily only — does not move the intraday floor                                                                       |
| Stooq                 | claimed       | every quote URL 404s and the CSV endpoint serves a JavaScript bot challenge                                                                    |
| Yahoo Finance `chart` | **yes, real** | 1124 bars at a 60s median gap, pricing AUD/USD in the range Pyth reports — but unofficial, and licensed for neither storage nor redistribution |

Yahoo is the instructive rejection: it is the only keyless source probed
that genuinely serves minute FX, so it fails on license rather than on
capability, and this roster is chosen on the right to *store* history.
Note also that it emits a gap-free minute grid padded with nulls — 1124
bars carrying 562 non-null closes — which makes it a grid source like
Twelve Data, never a zero-bar session detector.

### On-chain venue coverage, measured

The principle below decides *whether* a venue earns a feed. This is the
measurement it decides on: every roster stablecoin probed for its
on-chain venues on **2026-08-24**, keyless, through GeckoTerminal's pool
search — an aggregator in its sanctioned role of **venue discovery**
only. It locates pools; nothing downstream quotes from it.

Three caveats bound every figure here, and each one changed a reading
rather than merely qualifying it:

- **Totals are floors, not a census.** The search caps results per
  query, so a widely-listed token is truncated.
- **A shared ticker is not a shared token.** Matching is on symbol
  *text*. Eleven Solana pools priced a mint other than the roster's; all
  carried under $25 a day, so the headline figures survive that. `ZARU`
  is the separate and much larger case: its two pools are on Base and a
  Robinhood-routed venue, not Solana, they were created within two days
  of the probe, one is named `Zaru` rather than `ZARU`, and together they
  are its entire apparent $24k a day. **No genuine ZARU venue was
  found** — an absence from a capped search rather than a proof of
  absence, but the ticker mismatch and the creation dates make it a
  well-evidenced one.
- **Depth without flow is not liquidity.** Fifteen pools hold $50k or
  more against under $100/day. Two `EURCV` Solana pools report $66M of
  reserve on $4/day and an `XSGD` pool $138M on $4 — all three on
  non-roster mints, so they are valuation artifacts, not markets.

Columns: total 24h volume across every matched pool; then the single
**busiest** pool by 24h volume — its venue, pair and chain — and the
reserve sitting in *that* pool. Busiest is not always deepest, and AUDD
is the row where the two part company, so read the reserve column as the
depth of the busiest pool and never as the token's greatest depth.

| Token   | Ccy | 24h volume | Busiest pool's venue | Pair                | Chain     | That pool's reserve | Solana 24h |
| ------- | --- | ---------: | -------------------- | ------------------- | --------- | ------------------: | ---------: |
| `EURC`  | EUR |   \$31.07M | Aerodrome Slipstream | `EURC/WETH` 0.05%   | Base      |              \$683k |    \$3.05M |
| `XSGD`  | SGD |    \$1.89M | Aerodrome Slipstream | `XSGD/USDC` 0.01%   | Base      |              \$451k |     \$3.4k |
| `EURCV` | EUR |    \$1.73M | Uniswap v3           | `EUROC/EURCV` 0.01% | Ethereum  |             \$5.91M |         ~0 |
| `VCHF`  | CHF |     \$302k | ICPSwap              | `VCHF/ICP`          | ICP       |              \$485k |     \$145k |
| `AUDM`  | AUD |     \$156k | Uniswap v4           | `AUDM/USDT` 0.01%   | Ethereum  |             \$66.8k |        \$0 |
| `CADC`  | CAD |    \$71.8k | Aerodrome Slipstream | `CADC/USDC` 0.05%   | Base      |              \$133k |       none |
| `AUDD`  | AUD |    \$32.7k | First Ledger         | `AUDD/XRP`          | XRPL      |              \$3.0k |       none |
| `TGBP`  | GBP |    \$24.0k | Aerodrome Slipstream | `tGBP/USDC` 0.05%   | Base      |              \$293k |       \$84 |
| `MYRC`  | MYR |    \$23.4k | Uniswap v3           | `MYRC/USDT` 0.3%    | Arbitrum  |              \$125k |    \$14.6k |
| `IDRX`  | IDR |    \$15.8k | Aerodrome Slipstream | `IDRX/frxUSD`       | Base      |             \$81.6k |       \$68 |
| `EURAU` | EUR |    \$15.3k | Aerodrome Slipstream | `EURAU/USDC` 0.05%  | Base      |              \$141k |     \$2.3k |
| `BRZ`   | BRL |    \$11.6k | Oku Trade            | `BRZ/USDC.e` 0.05%  | Gnosis    |             \$40.7k |       none |
| `cNGN`  | NGN |     \$5.7k | Uniswap v3           | `USDT/cNGN` 0.01%   | Celo      |             \$57.8k |       none |
| `ZARP`  | ZAR |      \$992 | Uniswap v3           | `ZARP/USDC` 0.05%   | Ethereum  |             \$24.8k |        \$9 |
| `MXNe`  | MXN |      \$124 | Orca                 | `MXNe/CETES`        | Solana    |              \$180k |      \$113 |
| `VGBP`  | GBP |      \$111 | Raydium CLMM         | `VGBP/USDC`         | Solana    |             \$78.9k |      \$104 |
| `GYEN`  | JPY |       \$73 | Uniswap v4           | `GYEN/USDC` 0.008%  | Arbitrum  |              \$9.7k |       none |
| `TRYB`  | TRY |        \$3 | Trader Joe           | `TRYB/USDC.e`       | Avalanche |                \$66 |       none |
| `ZARU`  | ZAR |         ~0 | *none genuine*       | —                   | —         |                   — |       none |

**Two spec assumptions did not survive the measurement.** AUDD's "actual
on-chain settlement venue" is Aerodrome by *depth* — $36.5k of reserve
in AUDD/USDC against the XRPL pool's $3.0k — but not by *volume*: First
Ledger's AUDD/XRP turns over $25.4k/day against Aerodrome's $5.7k
combined. The claim is half right, and which half holds depends on
whether the question is where AUDD *sits* or where it *moves*. And CADC,
prioritized ahead of MXNe for this pass, returned **no Solana pool in
the probe at all** despite carrying a roster mint; 98% of its \$71.8k a
day is one Aerodrome pool. Per the first caveat that is a capped
search's silence rather than a proof — but it is silence where every
comparable roster token returned something.

**Three buckets, reasoning recorded.**

1. **Basis-leg price input candidates** — `EURC`, `XSGD`, `EURCV`, and
   marginally `VCHF`. These are the only tokens whose venues carry
   enough turnover to inform a basis leg. `EURCV` qualifies with an
   asterisk: its \$1.73M is mostly a `EUROC`/`EURCV` stable-to-stable
   pair, which prices one euro token against another and so speaks to a
   peg cross rather than to `token/fiat`. Cross-chain placement is no
   bar here — a Base or XRPL venue can be an input without being a
   competitor.
1. **Competitive signal — already trading at scale on Solana.** `EURC`
   above all: Orca's `EURC/USDC` does $3.05M/day across 12,836
   transactions on $366k of reserve. `VCHF` follows at $145k/day, with
   Raydium CLMM carrying $128k of it, then `MYRC`, whose \$14.6k is about
   62% of its entire global volume — the most Solana-native token on the
   roster, at a trivial absolute size. This read routes to customer
   development, not to the feed roster.
1. **Ignorable** — the remaining fourteen, too thin to be either.
   `EURC` and `VCHF` appear in both bucket 1 and bucket 2, so the five
   classified tokens are `EURC`, `XSGD`, `EURCV`, `VCHF` and `MYRC`, and
   19 − 5 leaves fourteen. `ZARU` is counted among them on the strict
   reading that a token with no venue found is trivially ignorable.

**The earns-a-collector rule, stated.** A venue becomes an ingestion
target only when its volume *and* depth make it a usable basis input
under consensus weighting. A pool turning over tens of thousands a day
is thin and manipulable: it earns little consensus weight, so collecting
it must be justified by there being nothing better — never by the pool
existing. Existence is the cheapest evidence available and this survey
found it nearly everywhere.

**The default expectation held.** On-chain volumes are small and
FX-relative pricing dominates. Of nineteen roster stablecoins, three
clear $1M/day and two more clear $150k. Twelve sit under $25k a day, and
six of those under $1k — which is not a market so much as a listing.

**Six markets have no measurable basis at all**, which generalizes the
MXNe finding rather than repeating it. `ZARP`, `MXNe`, `VGBP`, `GYEN`,
`TRYB` and `ZARU` each turn over under \$1k/day on-chain and reach no
CEX, so their basis leg has neither a venue print nor a second opinion.
An aggregator index is not merely uncorroborated for these — it is
describing a market that barely trades. Treat a first-reading band
breach on any of them as a configuration error before a market event,
because no market event there is large enough to be the explanation.

### The venue principle

**A venue gets a feed when its volume justifies the adapter**, and a
venue that gets one is a **pricing input** — never a benchmark to beat.
There is no adversarial framing anywhere in this stack: the question a
venue answers is "does this move the fair-value estimate?", never "can
we beat it?".

The corollary is that thin venues do not earn adapters. The Orca
EURC/USDC Whirlpool feed is **scrapped** on exactly this test — thin and
lagging, with Coinbase carrying the leading print — and the decoded-swap
ingestion decision it would have forced (a decoded-data provider vs.
archival-RPC decode) is moot with it. The same test applies uniformly to
every other venue where a covered token trades.

**Measured, that scrap needs restating on narrower grounds.** Orca's
EURC/USDC is not thin: $3.05M/day on $366k of reserve, the largest EURC
pool outside Base. The decision stands, but only on the *lagging* half —
Coinbase carries the leading print, and a venue that follows adds
nothing to a basis leg however much it turns over. Thinness was never
the operative reason, and citing it invites a re-litigation the volume
figure would win.

**The EVM adapter question is closed as not-needed-now.** Aerodrome on
Base is the roster's highest-coverage venue by a wide margin: the
deepest venue for EURC, XSGD, CADC, TGBP, IDRX and EURAU, and AUDD's
deepest USDC pool. If any single EVM adapter were ever built it is that
one. It still is not, and the reason is the rule above rather than the
cost. For EURC — the one token whose Aerodrome volume is unarguable —
the basis leg is already carried by Coinbase and Kraken; for every other
token Aerodrome's pools run from $70k/day down to $73/day, below the
bar. Base is an EVM transport with **no reuse of the Solana RPC
source**, so the adapter would buy a whole new transport for markets
that cannot use it. Aerodrome is therefore covered for **monitoring** by
the keyless polled analytics source this survey used, and quoting stays
on CEX basis plus the FX anchor. Revisit when a roster token begins
settling on Base at EURC's scale *without* a CEX to price it — that
combination, not Base volume alone, is what would flip this.

______________________________________________________________________

## 10. Polling discipline and the per-venue budget

Collectors and the maker share one host and one egress IP, and keyless
tiers rate-limit by IP — so venue quota is a shared resource that has to
be allocated rather than assumed.

- **One collector poller per venue per host.** Collectors own venue
  ingestion and never duplicate each other's polls. The maker's quote
  path is the **one deliberate exception** — it keeps its own
  independent polls (§7), drawn from the same budget, because quoting
  must not depend on a collector being up.
- **The maker's quote path holds first claim** on any venue's budget.
  Collection yields to quoting, never the reverse.
- **Batched symbol fetch wherever the venue supports it** — one poll
  returning N products replaces N per-market polls.
- **429 / `Retry-After` aware, with a per-source minimum interval**
  enforced in the shared HTTP client, so no individual source can
  outrun the budget.

| Venue / source          | Cadence                           | Claim                                  |
| ----------------------- | --------------------------------- | -------------------------------------- |
| FX anchor (streaming)   | Push; no poll                     | Maker first; collector taps the stream |
| CEX basis venues        | Slow poll, batched across symbols | Maker first                            |
| `coinbase` candles      | 15 s                              | Collector                              |
| `oanda` candles         | 60 s                              | Collector                              |
| `twelvedata` bars       | 300 s, widened by roster size     | Collector                              |
| `alphavantage` daily    | 6 h, widened by roster size       | Collector                              |
| `coinbase-ticker` ticks | 15 s per product                  | Collector                              |
| `kraken` ticks          | 15 s, batched across the roster   | Collector                              |
| `pyth` ticks            | 15 s, batched across the roster   | Collector                              |
| Issuer / peg rates      | Order of a day                    | Collector                              |
| On-chain (indexer RPC)  | Framework poll interval           | Indexer                                |

The FX session / week instants deliberately have **no row here**: they
are generated from a committed rule table rather than fetched from a
venue (`docs/market-calendar.md` §4), so they touch no external quota
and this section does not govern them. What they need instead is a
generated-through watermark, since their failure mode is a silently
expiring horizon rather than a rate limit.

**Why the Coinbase candles feed polls at a quarter of its bucket width.**
It used to poll every 60 s on 60-second candles, which meant the newest
closed bucket was discovered up to a full minute after it closed — the
series looked like it updated once a minute *and* lagged. Fifteen seconds
does not change the row rate, since 60 s is the finest bucket the endpoint
offers, but the newest closed bucket now lands ~45 s sooner. Four requests
a minute per product is nowhere near the venue's public-tier ceiling.

**A roster multiplies a metered venue's request count, and this is the
easiest thing here to get wrong.** A cadence sized for one pair is
multiplied by the number of pairs: Alpha Vantage's six-hour tick is four
requests a day for one pair and 28 for seven, against an account quota of
**25**. So the two metered FX collectors compute a floor from their roster
size and their usable daily share, widen the configured interval to it, and
log the effective value. Widening is nearly free for these venues because
their endpoints return a *window* of bars — one poll backfills everything
missed since the last, so a slower cadence makes a bar land later, never
absent.

That floor is the steady-state half; the shared client's minimum interval
(below) is the burst half. **On a metered venue both are needed** — Alpha
Vantage's client raises its floor to 1 h, 24 requests/day against a 25/day
account, which holds only because every feed in the process shares one client.
Seven independent clients would each allow 24/day, or 168 against a 25/day
quota, which is precisely why the roster collectors build one client and clone
it (§5).

Two venues need only the poll interval, and it is worth being exact about why
rather than reading the rule as universal: Coinbase and OANDA keep the shared
250 ms default *because* the default is already stricter than the venue's
documented rate, so a raised floor would buy nothing. Note what that does leave
open. Coinbase's public tier is rate-limited **per IP**, and this PR gives it
two collector processes — the candle feed and the ticker feed — which are
separate containers with independent pacers. The one-client-per-venue invariant
therefore holds *within* a process, not across the host. At the default rosters
that is roughly 8 requests a minute against a 600/minute ceiling, so nothing is
at risk; but a future roster large enough to matter would need cross-process
coordination that does not exist, and the honest statement is that this is
headroom rather than a guarantee.
Those cadences govern the **caught-up** state only — while a source
backfills, the runner loops without pausing and only the shared client's
minimum interval (below) paces it. That is the trap worth naming:
**steady-state polling and catch-up draw on the same budget but are
governed by different knobs**, so a cadence sized correctly for the
caught-up state says nothing about what a cold backfill will do. At the
250 ms default a backfill issues ~240 requests a minute — 30× Twelve
Data's 8/minute tier.

The resolution is that **every venue states its own floor, sized to its
own documented limit**, rather than most of them inheriting a default
that happens to be wrong for them. A floor set to the venue's real limit
makes the trap structurally unreachable: whoever adds the next pager
inherits a correct number instead of having to remember this section.
Recording the limit beside its adapter is half the point — the number is
what decays, so it lives in one place, cited.

**The alternative considered and rejected: a separate, slower backfill
interval.** Since the trap is that catch-up and steady-state polling draw
on one budget through different knobs, the obvious fix is a second knob —
a `backfill_interval` the runner sleeps while `!caught_up`. It was
rejected because the client already owns pacing, so this would create a
*second* pacing authority able to disagree with the first, and because it
does not actually remove the trap: a backfill interval still has to be
set correctly per venue, so the failure mode becomes "someone forgot the
second knob" rather than "someone forgot the first". Per-venue-correct
floors need no new knob and cannot be forgotten, because the floor a
pager inherits *is* the venue's limit. Revisit only if a venue ever needs
catch-up paced genuinely differently from steady state — a real
difference in kind, not a difference in number.

| Venue                   | Documented free / keyless limit  | Floor   |
| ----------------------- | -------------------------------- | ------- |
| Coinbase (public)       | 10 req/s, burst 15, per IP       | default |
| OANDA                   | 100 req/s                        | default |
| Kraken (public)         | ~1 call/s, counter-based         | 1.2 s   |
| Pyth Hermes             | 10 req per 10 s per IP           | 1.2 s   |
| CoinMarketCap (keyless) | unpublished; per-IP pooling, 429 | 2 s     |
| Frankfurter             | unpublished; soft fair-use       | 1 s     |
| Twelve Data             | 8 req/min (800 credits/day)      | 8 s     |
| CoinGecko (keyless)     | 5–15 calls/min, dynamic          | 15 s    |
| Alpha Vantage           | 25 req/day, whole account        | 1 h     |

**Each floor sits strictly inside its venue's limit, not on it.** Pyth's
1.2 s is 8.3 requests per 10 s against a documented 10, CoinGecko's 15 s
is 4/min against a 5/min low end, and Kraken's 1.2 s is 0.83/s against a
documented ~1/s — each deliberately short of the arithmetic maximum
(1 s, 12 s and 1 s respectively would hit the cap exactly). Three things
make the exact-cap value the wrong choice: whether a venue meters a fixed
or a sliding window decides whether a boundary request is the last
allowed or the first refused; the limits are per **IP**, so a second
process on the host is already over; and one retry after a transient
failure adds a request the arithmetic never counted. The per-venue tests
assert a strict inequality for exactly that reason.

Two venues keep the 250 ms default because it is *already* stricter than
what they allow: **Coinbase** at 10 req/s (the default is 4, so a paged
candle backfill running flat out sits 2.5× inside the limit — the sharp
case that turned out not to be sharp) and **OANDA** at 100 req/s, where
the default's 4 req/s is 25× stricter than asked. Everything else raises
its own.

Two of those numbers are **ours, not the venue's** — Frankfurter and
keyless CoinMarketCap publish no rate — and both are marked as such at
the adapter so nobody later mistakes a judgement call for a citation.

Note what a floor can be sized against at all: a **rate**. Three venues
express their free tier as a **quota** instead — Alpha Vantage 25/day,
Twelve Data 800 credits/day, and CoinMarketCap's *keyed* plan
15,000/month — and a minimum interval cannot enforce one. Two of the
floors above are consequently doing something weaker than the rest:
Alpha Vantage's hour is standing in for a quota outright (the venue
publishes no rate), and Twelve Data's 8 s holds its 8/min rate while only
the collector's cadence holds its daily credits.
It is in-process state: it paces requests while the process is up and
resets when it restarts, so an interval chosen to satisfy a daily budget
holds only across a single continuous run. A crash-loop, or a few local
stack cycles in an afternoon, exhausts the quota while every individual
pacing decision stays correct — and the gap is invisible precisely
because the steady-state arithmetic checks out.

Nothing here closes that gap; a durable per-venue counter beside the feed
cursors would, and is not built. What the roster does instead is prefer
the route that has no quota to breach: CoinMarketCap is on its keyless
public endpoint for exactly this reason (§4), which is also why the
maker's feed cascade needs no credential at all.

The three FX cadences span two orders of magnitude, and the reason is
worth stating because it is counter-intuitive: **a tight request budget
constrains poll frequency, not bar width.** These are OHLCV *window*
endpoints — one request returns many bars — so Twelve Data on 800
credits/day still yields a continuous 60-second series; it just arrives
in less frequent batches. A 60-second tick there would spend 1440
credits and exhaust the account before the day was out, so 300 s (≈288
requests/day, about a third of the budget) leaves room for restarts and
a backfill running alongside. Alpha Vantage's 25 requests/day is the
whole account, and a daily bar changes once a day, so six hours buys
everything a tighter tick would.

The maker is **built to be one multi-market process**, which is what makes
batching natural: a single poll serves every market it quotes, and that is
the shape the compose stack runs (one `maker-bot` service, no `--market`).

**The TUI runs it the other way, and that is a multiplier to keep in
view.** So the demo can start and stop markets individually, it launches
one process per market (`--market <symbol>`, `tui/src/bot.rs`), which
means up to seven makers each polling every venue on their own — seven
times the request rate the single-process arithmetic above assumes, from
one egress IP. Nothing in the client can see across process boundaries to
correct for it (see the cross-process note below): a per-client floor is
per *process*, so seven processes get seven floors.

**CoinGecko is the one venue where that shape does not clearly fit, and
the number is worth stating rather than rounding off.** Seven makers at a
60 s cadence is 7 polls/minute against a keyless tier documented as a
*dynamic* 5–15/minute — comfortably inside the top of that band and
around 40% over the bottom of it. Which end applies is not ours to know,
so the honest description is that the per-market demo shape can be
throttled here, by design tolerance rather than by accident: a 429 records
its cooldown and surfaces (below), the basis leg then falls through to
CoinMarketCap — keyless, so there is no quota being spent on the retry —
and the FX anchor is a different venue entirely and unaffected. The cost
of the tightest case is a degraded basis tier for a cooldown, not a dark
market. Every other venue in the roster fits the seven-process shape with
room to spare.

A local price fan-out — one poller feeding N quoting tasks — is what would
remove the multiplier properly. That is the trigger to revisit this if the
per-market shape ever outgrows the demo, not a problem to pre-solve.

### How the shared client enforces it

Every venue adapter reaches the network through one
`HttpClient::get_json` (`feeds/src/http.rs`), so the bounds below hold
for all of them at once rather than per adapter.

- **A minimum interval per client**, 250 ms by default and raised per
  venue with `with_min_interval` (the table above gives every venue's).
  It is a floor on back-to-back requests — a paged backfill, a burst
  after an outage — not a cadence: steady-state polling rate stays with
  the runner's `poll_interval`. Clones share one gate, so a cloned client
  draws on the same venue budget instead of opening a second one, and an
  idle stretch banks no credit for a later burst.
- **The gate is per client, so a venue polled by several sources needs
  one client cloned across them.** This is the sharp edge of the bullet
  above, and it cuts the other way: two *independently constructed*
  clients for one venue each pace correctly on their own while together
  spending double, because a keyless limit is per IP and neither knows
  about the other. Coinbase's ticker is the case in the tree — its
  endpoint is per product, so a roster of N tokens is N sources — and the
  maker builds one client and hands each source a clone
  (`CoinbaseTicker::from_client`). Reach for `from_client` (or the
  equivalent seam) whenever a venue gets more than one source in a
  process; `new` opening its own client is the convenience path for the
  single-source case.
- **A 429 records a cooldown and surfaces the error.** The venue's
  `Retry-After` — the delta-seconds form; an HTTP-date falls through to
  a 60 s default — is clamped to five minutes and held as the earliest
  instant the next request may go out. The runner then logs it, reports
  it to the metrics seam, and backs off. Rate-limit pressure therefore
  stays visible instead of being absorbed by a silent in-call retry, and
  a cooldown far longer than the 10 s request timeout never sits inside
  a single call.
- **A response-body cap** of 8 MiB, raised per venue with
  `with_max_response_bytes`. A declared `Content-Length` over the cap is
  refused before a byte is buffered, and the running total then catches
  a venue that under-declares or omits it. This gives the client a size
  bound to go with the time bound the request timeout already provided,
  so a wedged or hostile endpoint cannot make a consumer allocate
  without bound.

The one bound left to the operator is the cross-process half: nothing
in the client stops a *second process* from polling the same venue, so
the single-poller invariant above stays a deployment property.

______________________________________________________________________

## 11. Analytics over the collected history

All run in SQL over Postgres, each reading the tables of §8 and writing
its own. These are the numbers the maker spec defers to:

- **Basis-process characterization, per market** — mean, volatility,
  half-life of reversion, stationarity (whether it mean-reverts or
  random-walks), and jump / tail frequency. This sets the basis EMA
  smoothing half-life and the per-market sane band that
  [`market-making.md`](market-making.md) §1 and §4 leave TBD.
  Whether a basis mean-reverts at all is the assumption the maker rests
  on: strong for tokens with live redemption arbitrage, weak or absent
  for thin exotics, and never assumed across the roster.
- **Realized volatility by market and time-of-day** — the σ input to
  the quote ladder and its per-level time-in-force, so the ladder is
  shaped to the market's measured volatility instead of a calm-σ
  assumption.
- **Flow regimes** — tag every window by FX session (Sydney / London /
  NY, with the London–NY overlap the peak), the weekend / overnight gap
  (interbank FX closed Fri ~5pm → Sun ~5pm ET while crypto keeps
  trading), scheduled macro events, and depeg / stress episodes (the
  regime where the basis stops mean-reverting and the maker is most
  exposed). Every other statistic here is sliced by regime.
- **Lead-lag** — cross-correlation of FX spot against the CEX token
  print and the issuer rate, to settle which signal leads. This sets the
  staleness thresholds, and the session boundaries the weekend anchor
  switch fires on, that [`market-making.md`](market-making.md)
  §1 leaves TBD.
- **Observability — realized-vs-modeled basis error.** The gap between
  the live basis and the engine's EMA estimate of it is the standing
  signal the deployed maker is monitored on; the collected history is
  what sets its healthy band and alert threshold, so the bot runs
  against a measured baseline rather than blind.

### What is implemented today

The first slice of the above ships as committed SQL in
`market-data/analytics/` (see its README) plus an **FX analytics**
dashboard beside the ingestion one, in the same provisioning tree.
Four queries: the basis against an FX anchor in bps, and — the three
that need only the venue's own candles — weekend versus weekday,
behavior by FX session, and realized volatility by hour and regime.

The dashboard is ordered **raw first**: the plain price series for one
selected venue and product, beside a table of what the store holds per
source and product, then the three regime panels below. The basis is a
committed query but deliberately **not** a panel in this pass — a
two-leg panel is only meaningful when both legs are the same currency
family, nothing enforces that yet, and an anchor picker that falls back
to another currency's rate renders a large, stable, meaningless number
rather than failing visibly. Grouping products by currency family, which
is what would let the pairing be checked rather than trusted, is a
schema change tracked separately.

Being single-leg is what let three of the four produce results before
any FX feed existed: a cold collector backfills 60 days, so the history
is on disk minutes after first start rather than accruing over calendar
time. Only the basis series needs the anchor leg; it returns no rows
until one is collected, rather than failing.

Two invariants hold across all of them, and both exist because
violating them yields a confident wrong answer rather than an error:

- **Adjacent-bucket returns only.** A log return is computed between two
  buckets only when they are genuinely consecutive at the stated
  granularity. A venue emits no candle for a minute with no trades
  (~12% of bars on a 60-day EURC-USDC backfill) and interbank FX leaves
  a ~48-hour weekend hole, so an unguarded window function bridges the
  gap and reports one enormous pseudo-return attributed to whichever
  bucket sits on the far side.
- **Local wall clock, never fixed UTC offsets.** Session windows and the
  FX week boundary resolve per timestamp in the relevant centre's own
  zone. Daylight saving moves these twice a year and the hemispheres
  shift in opposite directions, so a hardcoded UTC hour is wrong for a
  large part of any multi-month window — which is the window these run
  over.

Measured on `coinbase` / `EURC-USDC` / 60s over 2026-06-15 to
2026-08-14, the shape of the result: weekends are calmer (0.98 vs 1.42
bps per bar) with a far smaller tail (largest single-bar move 13 vs 82
bps); the session rhythm survives into the stablecoin (London 1.58 and
New York 1.57 against Sydney 1.05 and Tokyo 1.13); and the intraday
profile carries FX's fingerprints, peaking at 13:00 UTC in the
London/New York overlap with a second spike near the 21:00 UTC daily
roll, neither of which appears on weekends.

The basis itself became measurable once an FX anchor existed for a
currency with a liquid venue leg. Over 48,668 aligned minutes,
`coinbase` / `EURC-USDC` tracks `oanda` / `EUR-USD` to a mean of
**+0.16 bps** with a standard deviation of **1.86 bps**, reaching −87
and +65 bps at the extremes — a tight peg with fat tails, which is the
structure a basis series should have. The same measurement on the AUD
pair returns 26 points and a mean of +22 bps: the anchor there is sound
and the venue leg is what starves it, so that figure reflects a thin
tape rather than a wider basis.

**EUR/USD leads the analytics; AUD is carried as a case study.** The
reason is measured rather than editorial — over the same window and
venue, `EURC-USDC` recorded 71,469 bars (1,172/day) against
`AUDD-USDC`'s 845 (14.1/day, thinnest day 4). A book printing in ~1% of
minutes supports a daily-resolution basis level and not an intraday
session or volatility read. Every query takes source and product as
parameters, so nothing is rewritten if that book deepens. The thinness
is itself worth keeping: a barely-traded centralized book is the
clearest argument for an on-chain FX market in that currency.

One hazard to carry into the anchor work. FX vendors disagree about
whether a weekend exists — some publish a complete 1440-bar Saturday,
others correctly publish nothing — so both conventions will appear in
`cex_prices` under different sources. Do **not** compute a weekend
volatility figure from a source whose market was closed; it yields a
plausible ultra-low sigma for a session that never traded and fails
silently. Prefer a source whose weekend bar count is zero. Do still
compute the weekend **deviation** series: an anchor holding flat while
the venue leg moves is the mechanism being measured, since with no
arbitrage channel open the basis is free to widen.

Resist detecting such a series by a collapse in distinct closing
prices. That heuristic was tested against this repo's own traded tape
and is wrong — a real Saturday carries 3–15 distinct closes across
~1,000 bars, and weekends show *more* distinct closes per bar than
weekdays (8.9 vs 5.5 per 1,000). Sparse pricing is what a quiet real
market looks like.

______________________________________________________________________

## 12. Deployment — local demo and cloud

**`make demo` flies the full production stack locally** — the production
maker, the collectors, the shared database, the dashboards, and the TUI,
from the same compose file and the same images the cloud host runs. Test
like you fly: there is no demo-only feed tier and no demo-only bot code,
so the thing demoed is the thing deployed.

That compose file is `infra/localnet/docker-compose.yml`, and it holds
exactly one Postgres. The collectors used to run a second one in their own
compose file; consolidating them is what makes a cross-boundary join a
query rather than an export. Its data sits on a **named volume**: the
indexer could always re-sync from chain, but a CEX backfill window is
finite, so collected price history that scrolls out of it is
unrecoverable. Stopping services keeps the volume; only an explicit
`down -v` (`make clean-docker`) discards it.

**The dashboards are configuration, not a service to write.** Grafana
OSS joins the same compose file (`market-data/grafana/`), provisioned
entirely from the repo — datasource, dashboard loader, and the dashboard
JSON — and mounted read-only, with no volume of its own. So there is no
Grafana state to deploy or back up: the tree that renders the local
ingestion dashboard is the tree that renders the cloud one, and every
value that differs between them is an environment variable. Which makes
the stage-1 EC2 box below a credentials change, exactly like the store
sink's move to Aurora.

**AWS is ephemeral by default.** Every stack proves the full
deploy → verify → teardown cycle and is torn down after; nothing is left
running because it might be needed. The shared database is the **one
deliberate always-on exception**, because a gap in collection is
unrecoverable in a way an idle compute stack never is.

Its placement is staged:

1. **EC2 running the compose stack, with scheduled dumps to S3.** Cheap,
   identical to the local shape, and enough to start recording.
1. **Aurora Serverless v2** once recording-for-real starts, with S3 as
   the archival tier. The store sink targets a `PgPool`, so this is a
   connection-string change and a restore — one engine, one `sqlx` and
   migration toolchain, no dialect port.

**A hosted collector gets environment variables, not files.** ECS supplies
configuration as environment variables (and secrets as references resolved
into them); there is no practical way to hand a task a configuration file
short of baking it into the image — which puts a rebuild back in the way of
every change — or fetching it from S3 at startup, which is a new dependency
and a new failure mode for something read once. So **collector
configuration must be expressible as environment variables, or live in the
shared database**, and that constraint decides where each kind of
configuration goes:

- **Scalars and short lists** — a base URL, a cadence, a product roster —
  are environment variables. `PRODUCT_IDS` is a comma-separated list for
  exactly this reason.
- **Anything structured, or anything an operator should change without a
  deploy**, goes in the database. The Pyth FX roster is the first of these
  (§8): a cross needs a 32-byte hex id that no rule derives from a product
  id, so the options were a compiled constant, a base64 blob in a variable,
  or reference data in the store the collector already must reach. Adding a
  cross is now an `INSERT` plus a restart.

The database option is only available to a **DB-primary** app — one that
already cannot work without Postgres (§8). It is not open to the maker's
quote path, which keeps Postgres a soft dependency by design, and that is
why the maker's copy of the FX roster stays a compiled constant serving as
its degraded-mode fallback. Two copies of those coordinates therefore exist
on purpose, and a test asserts the seed and the constant agree so the
duplication cannot drift silently.

### Credentials — the local secrets enclave

**1Password is the local mock of AWS Secrets Manager.** One provider
interface (`feeds/src/secrets.rs`), one backend per store, and the same
secret names in both — so a collector that resolves a key locally
resolves it the same way once deployed, and configuration carries
*where to look* rather than the secret itself.

Every credential has one **canonical name**, `<provider>/<secret>`: the
party that issued it, then which of its credentials this is.
`oanda/api-key` is the OANDA key no matter who reads it, so a key the
collectors and a bot share has one name, one entry per store, and one
place to rotate. Naming a secret after its consumer
(`market-data/oanda-key`) would force a rename the moment a second
consumer appeared, and a rename that lands in one store but not the
other is a silent outage.

Each store only **prefixes** that name — nothing is translated, so no
store needs a mapping table and the spellings cannot drift:

| Store               | Key for `oanda/api-key`      |
| ------------------- | ---------------------------- |
| process environment | `OANDA_API_KEY`              |
| 1Password           | `op://<vault>/oanda/api-key` |
| AWS Secrets Manager | `dropset/oanda/api-key`      |

**The 1Password mapping needs no escaping because the hierarchy lives
in an item's fields, not its title.** A secret reference parses as
`op://<vault>/<item>/[<section>/]<field>`, so a slash inside an item
*title* is not quotable — it re-segments the reference and the item
stops resolving. Measured against an item whose title contains one:

```text
$ op read 'op://<vault>/Vercel / v0/<field>'
[ERROR] could not get item <vault>/Vercel : "Vercel " isn't an item
```

That rules out storing a hierarchical name as a title — but not
hierarchy itself. An item's fields are addressable by label, which is
exactly the two levels `<provider>/<secret>` needs, so the vault holds
an item per provider and a named field per credential, and the
canonical name is already a valid reference tail:

```text
<vault>
├── oanda          api-key · account-id
├── twelvedata     api-key
└── alphavantage   api-key
```

A second credential from a provider already present is a field on that
item rather than a new entry. Adding one an app actually resolves is a
field, a line in the operator file, **and** a small code change — the
`SECRET_NAME` constant beside its adapter, the call site, and the roster
in `market-data/tests/secrets_example.rs` that keeps the template and
the constants from drifting apart.

**Setup is one git-ignored file.** Copy
`infra/localnet/secrets.local.env.example` to `secrets.local.env` and
replace `<vault>` with your own vault's name. That file holds
references, never values, and it is the *only* place a real vault or
item name appears — the tracked template carries placeholders, which is
why every example here is written `op://<vault>/…`. The shell profile
is deliberately not a secrets channel: no **credential** is ever
exported to run the stack.

**Do not `source` that file.** It holds `op://` *references*, and a
reference is not a credential — sourcing it puts the reference string
itself into `OANDA_API_KEY`, where the environment backend would find a
perfectly non-empty value and hand it to the venue as an API key. The
provider refuses an `op://` value outright for exactly this reason, so
the mistake surfaces as a startup error naming it rather than as a 401
from a vault that was never consulted.

Three paths resolve from there, in the order the provider consults
them:

1. **The process environment**, always first — the override path, and
   what CI uses. No `op`, no vault, no 1Password dependency in CI.

1. **The containers**: `make fx-collectors-up` wraps the compose
   invocation in `op run`, which resolves the references and exports
   them under the derived variable names. A container never reaches a
   secret store itself — it has no `op` and no session, and is handed
   resolved values. That is the same shape the hosted deploy has, where
   the instance role fetches from Secrets Manager.

1. **A collector run straight from the host**, which resolves through
   `op read` per key — no credential exported, and no `op run`:

   ```sh
   DROPSET_OP_VAULT=<vault> cargo run --bin market-data-oanda
   ```

   The provider reads that one variable from its **environment**; it
   does not parse the operator file, which is why the vault is named on
   the command line here. (`op run --env-file=… -- cargo run …` works
   too, and is the better habit if the file also pins an account.)

Resolution is **fetch-once-at-startup**: a backend is consulted while a
binary wires itself up, never per request, so neither the `op`
subprocess nor the eventual Secrets Manager round trip sits on a poll
path. An empty value is treated as absent rather than as a secret,
because the compose services pass credentials as `${VAR:-}` — without
that, a shell that had ever sourced them would shadow the vault that
does hold the key.

Two things are deliberately **outside** this enclave. The maker's
**signing keys** stay with the hosted-custody work — an instance-role
fetch, never a local vault — and the maker's CoinMarketCap key still
reads its own variable; its canonical name is declared with the adapter
so that migration is a call-site change when it comes.

______________________________________________________________________

## 13. Open questions

- **Live-sink backpressure.** *Resolved — the maker bot (§6) is the
  first live-sink consumer.* The bounded broadcast that drops to the
  latest is the policy: the maker's tick keeps only the freshest reading
  per price tier and the highest-`nonce_after` fill per market, so a
  lagged receiver loses nothing the reconcile needs and a slow bot never
  stalls a source shared with a store sink.

- **Streaming adapter phasing.** *Resolved.* The first streaming adapter
  is the RPC `logsSubscribe` fill socket, landed as the maker bot's fill
  feed through the `ChannelSource` stream seam (§4). A CEX socket for
  the basis leg follows when polling it at the §10 cadence proves too
  slow for quoting — not before.

- **Backfill windowing.** *Resolved — the framework owns a paged
  backfill (`feeds/src/backfill.rs`) that any poll source can adopt; the
  RPC source is the first.* (It is a helper a source drives, not
  behavior a source inherits: the HTTP venue adapters page their own
  backfill and are unchanged.) The correction that matters is
  directional: a resume cursor is an exclusive *lower* bound, so
  advancing it to a mid-backlog position discards everything older
  rather than deferring it. The pager therefore withholds every page
  until the backward walk has reached the bound, then emits
  oldest-first. To make a walk that must finish affordable it stores a
  **page key** per page rather than the records — for the RPC source,
  the `before` marker *and* the newest signature the page held when it
  was enumerated. Both ends are needed: a window bounded only from below
  is open at the tip, so it can grow between enumeration and emission,
  and the cursor must land on the recorded newest rather than on
  whatever is newest by then. The per-poll page cap bounds how long one
  `next()` runs, not how deep the walk may go: an unfinished walk
  resumes where it stopped. Emission is two-phase for the same reason a
  cursor is conservative — the page stays queued until the batch is
  built, so a source error retries it instead of skipping it.

- **Observability hook.** *Resolved — `FeedMetrics`, a two-callback
  trait (`on_batch` / `on_error`) with no-op defaults that the runner
  emits through; `run_with_metrics` / `run_until_with_metrics` carry a
  recorder, and the plain `run` / `run_until` keep their signatures.*
  Each batch reports records, `caught_up`, and the fetch and dispatch
  durations; error rate is `on_error`'s frequency against `on_batch`'s.
  Cursor **lag** is deliberately not reported as a number: only a source
  knows whether its position is a timestamp, a slot, or a signature, so
  the framework exposes `caught_up` and leaves the lag derivation to the
  recorder. The indexer is the first consumer.

- **FX bar source.** *Resolved — three free-tier vendors, no paid one
  needed (§9 "The free-tier FX roster").* The question assumed the
  choice was Pyth Benchmarks against a paid vendor, and the cost turned
  out to be zero: OANDA's practice tier issues a free v20 token and
  serves minute bars back three or more years, which is deeper than any
  consumer has asked for. It is the anchor; Twelve Data is an
  independent minute-bar cross-check and Alpha Vantage a daily
  corroboration, both free. History depth — the half of this question
  that mattered — is therefore not a constraint at all.

  Two things this resolution *did* settle that the question did not
  anticipate. **Symbol spelling has to be ours**: the vendors disagree
  three ways, so the stored `product_id` is canonical and the venue's
  form is derived. And **weekend coverage is a vendor convention, not a
  fact about the market**: OANDA publishes nothing, Twelve Data
  publishes a complete grid, so session detection reads bar absence from
  the former rather than trusting either alone.

  What remains genuinely open is narrower: whether a *streaming* FX
  source (Pyth Hermes, already wired, or an OANDA price stream) should
  also feed the store rather than only the maker. That is a latency
  question for the quote path, not a history question, and it waits on a
  consumer that needs sub-minute FX.

  **Half of that is now answered**: the tick tier (§9) polls Pyth into
  the store on a batched roster, so the Pyth-into-the-store question is
  settled and only the OANDA price stream is still open.

  The related "is a third intraday source needed" question is
  **resolved** (§9 "The free-tier FX roster"): Pyth is the third, it is
  already wired, and the shortfall is five currency feeds left unwired
  rather than a missing vendor. What that resolution leaves open is one
  measurement, not a decision — **per-currency intraday coverage for
  OANDA and Twelve Data across the roster** is verified only for AUD and
  EUR. It needs one request per currency per vendor, and it is
  **deferred rather than blocked**: the resolver in
  `feeds/src/secrets.rs` reaches both keys today, given
  `DROPSET_OP_VAULT` naming a vault and an authenticated `op`. What it
  wants is an environment carrying that variable, which is a
  session-launch concern and not a missing capability — the hosted
  Secrets Manager backend is still pending, and nothing here waits on
  it. Pyth's column of that matrix is measured and complete above; the
  other two columns are the gap. Until they are filled, MYR and NGN are
  the named at-risk currencies on the one column that could be measured.

- **Econ-calendar source.** *Resolved — the dataset is deferred, so
  there is no feed to choose (`docs/market-calendar.md`).* The question
  assumed a macro calendar was needed; the market-calendar research
  established that it answers neither question the calendar exists for
  (is a leading FX feed expected now, and which sessions overlap), and
  that the hour-of-day volatility profile already absorbs the habitual
  08:30 ET release — the 08:00 ET hour is the stored series' most
  volatile. That last point is *qualitative*: the release hour and the
  London/New York overlap open are the same hour, and the collected
  window cannot separate them, so it supports deferring a macro
  calendar without quantifying what one would add.

  What the calendar does need — the FX session and week instants — is
  **generated** from a committed rule table through Postgres, not
  ingested, so no adapter, sink, key, or venue budget is involved.

  Two findings worth keeping even though the dataset left scope, since
  both cost a probe: **BLS blocks automated fetches** (HTTP 403 on the
  iCalendar and HTML schedules alike, browser user-agent included,
  while egress elsewhere succeeds), and the **Federal Reserve
  publishes no FOMC calendar JSON API**. If macro ever returns, FRED is
  the viable route — it republishes the BLS schedules and needs one
  free key — and its release dates are believed date-only, so times of
  day would be authored. Revision behavior and forward horizon were
  never established for any source.

- **History depth before the estimates are significant.** The §11
  characterizations need enough repeats of each regime — weekend gaps,
  macro events — to mean anything. The retired survey guessed a 60–90
  day backfill for a one-shot gate; a standing collector instead needs a
  stated depth per estimate, below which the number is reported as
  provisional rather than used to set a band.

- **Retention.** How much history stays in Postgres before it rolls to
  the S3 archival tier, and whether the analytics read across the seam.
