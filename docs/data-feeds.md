<!-- cspell:word backfilling -->

<!-- cspell:word backpressure -->

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
  returning a `symbol → USD` map. CoinGecko, CoinMarketCap,
  ECB/Frankfurter, and Kraken are the four today. This is the per-venue
  budget's main lever (§10): one poll for N markets rather than N polls.
  A symbol the venue does not quote is omitted, never an error — one
  unlisted token must not dark the rest of the roster.

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
comes from* stays a deployment decision the consuming app owns — the
maker reads `CMC_API_KEY` from its own config today, and a secrets
provider can supply it later without the adapter changing.

**A credential rides the transport as a sensitive header.** An adapter
that authenticates with a header passes it to
`HttpClient::with_secret_header`, never plain `with_header`: the value is
marked sensitive, which keeps it out of any `Debug` render of the header
map and out of HTTP/2's HPACK dynamic table. This is a constructor-level
guarantee on purpose — whether a key can leak must not depend on which
types happen not to derive `Debug` yet. Plain `with_header` remains the
right call for a benign header, such as OANDA's UNIX datetime-format
preference, where debug visibility is worth keeping.

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

______________________________________________________________________

## 6. Consumers and boundaries

- **Market-data collectors (store sink).** The collectors build their
  sources on this crate into a store sink: an HTTP Coinbase reference
  feed first (the proof feed), then the FX, issuer-rate, and
  econ-calendar feeds. See §7 onward.
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

| Table                                                              | Writer             | Contents                                            |
| ------------------------------------------------------------------ | ------------------ | --------------------------------------------------- |
| `feed_cursors`                                                     | `feeds` store sink | Resumable per-feed position (JSONB)                 |
| `cex_prices`                                                       | market-data        | CEX reference candles, per venue and product        |
| `fx_rates` *(planned)*                                             | market-data        | Fiat-cross bars for the FX anchor leg               |
| `peg_rates` *(planned)*                                            | market-data        | Issuer / redemption reference rates                 |
| `fx_events` *(planned)*                                            | market-data        | Economic-calendar event times                       |
| `basis_series` *(planned)*                                         | market-data        | Derived per-market basis series                     |
| `vol_estimates` *(planned)*                                        | market-data        | Realized volatility by market and window            |
| `regimes` *(planned)*                                              | market-data        | Regime tags every other stat is sliced by           |
| `fill_events`, `events`, `takes`, `market_stats`, `indexer_cursor` | indexer            | On-chain event capture and its rollups              |
| Maker parameter and telemetry tables *(planned)*                   | maker go-between   | Slow parameters published to the bot; run telemetry |

Adding a table means naming its writer here. A table with two writers is
a design error, not a configuration choice.

The one carve-out is `feed_cursors`, which the **framework** owns rather
than any single app: every store-sink process writes its own row, keyed
by the feed name. Writers partition by key, so the rule holds at the row
level even though several apps touch the table — and the framework, not
an app, defines its shape. A table wanting that treatment has to earn it
the same way: a key that makes the partition structural, not a
convention two writers agree to keep.

______________________________________________________________________

## 9. Sources and venue policy

Fair value is **`fx_rate × basis`** — a deep, exogenous FX anchor
corrected by a slow, thin stablecoin basis, decomposed across both pegs
(`basis = (token / fiat) ÷ (USDC / USD)`). The model itself, its
regimes, and its failure modes belong to
[`market-making.md`](market-making.md) §1; what follows is which
sources feed each leg and on what terms.

| Role                             | Source                                                                            |
| -------------------------------- | --------------------------------------------------------------------------------- |
| FX anchor (`fiat/USD`)           | Pyth Hermes FX + the FX roster below, all wired; ECB / Frankfurter daily fallback |
| Basis (`token/fiat`, `USDC/USD`) | Coinbase `<token>/USDC` (wired), Kraken `<token>/USD` (wired)                     |
| Peg truth                        | Kraken `USDC/USD` (wired); Circle / issuer redemption rate                        |
| Token/USD, last resort           | CoinGecko / CoinMarketCap — reflexive, never the anchor                           |
| Macro overlay                    | Econ-calendar loader (ECB / FOMC / CPI / NFP times)                               |

### What is wired, and why the rest is not

The primaries above are keyless and live. The gaps are not oversights — each
was probed and ruled out on evidence:

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
  issuer-rate proxy — but nothing subscribes to it yet: the maker's roster
  asks only for `<token>/USD` plus the shared `USDC/USD`. A credentialed
  Circle Mint feed supersedes both when keys exist.
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

### The free-tier FX roster

Three FX vendors are wired, each on a free credential. What each is
*for* follows from what its free tier actually serves, measured against
a live key rather than read off a pricing page:

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

| Venue / source         | Cadence                           | Claim                                  |
| ---------------------- | --------------------------------- | -------------------------------------- |
| FX anchor (streaming)  | Push; no poll                     | Maker first; collector taps the stream |
| CEX basis venues       | Slow poll, batched across symbols | Maker first                            |
| `oanda` candles        | 60 s                              | Collector                              |
| `twelvedata` bars      | 300 s                             | Collector                              |
| `alphavantage` daily   | 6 h                               | Collector                              |
| Issuer / peg rates     | Order of a day                    | Collector                              |
| Econ calendar          | Order of a day, static download   | Collector                              |
| On-chain (indexer RPC) | Framework poll interval           | Indexer                                |

Those cadences govern the **caught-up** state only — while a source
backfills, the runner loops without pausing and only the shared client's
minimum interval (below) paces it. That is the trap worth naming:
**steady-state polling and catch-up draw on the same budget but are
governed by different knobs**, so a cadence sized correctly for the
caught-up state says nothing about what a cold backfill will do. At the
250 ms default a backfill issues ~240 requests a minute — 30× Twelve
Data's 8/minute tier.

So the two FX sources stricter than that default raise their own floor at
construction: **Twelve Data to 8 s** (7.5/minute, and about two minutes'
overhead across a whole 60-day backfill) and **Alpha Vantage to 1 h**
(24/day against its 25/day account). OANDA needs none — at 100 req/s
allowed the default is already ~400× stricter than the venue asks.

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

The maker stays **one multi-market process**, which is what makes
batching natural — a single poll serves every market it quotes. If
per-market maker processes are ever split out they would need a local
price fan-out; that is the trigger to revisit this, not a problem to
pre-solve.

### How the shared client enforces it

Every venue adapter reaches the network through one
`HttpClient::get_json` (`feeds/src/http.rs`), so the bounds below hold
for all of them at once rather than per adapter.

- **A minimum interval per client**, 250 ms by default and raised per
  venue with `with_min_interval`. It is a floor on back-to-back
  requests — a paged backfill, a burst after an outage — not a cadence:
  steady-state polling rate stays with the runner's `poll_interval`.
  Clones share one gate, so a cloned client draws on the same venue
  budget instead of opening a second one, and an idle stretch banks no
  credit for a later burst.
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

- **Econ-calendar source.** Which static feed for the ECB / FOMC / CPI /
  NFP times.

- **History depth before the estimates are significant.** The §11
  characterizations need enough repeats of each regime — weekend gaps,
  macro events — to mean anything. The retired survey guessed a 60–90
  day backfill for a one-shot gate; a standing collector instead needs a
  stated depth per estimate, below which the number is reported as
  provisional rather than used to set a band.

- **Retention.** How much history stays in Postgres before it rolls to
  the S3 archival tier, and whether the analytics read across the seam.
