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
stream seam (§4). The collector crate still sits at its
original path under `analytics/`; its move to `market-data/`, the
relocation of the venue adapters into `feeds/`, and the indexer's
migration onto the framework are separate tracked tasks.

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

- **Indexer — durable RPC poll.** `RpcPollSource`
  (`indexer/src/ingest.rs`) polls `getSignaturesForAddress` +
  `getTransaction` at `finalized` and returns base58-decoded
  inner-instruction blobs; `Store` (`indexer/src/store.rs`) is the
  `sqlx` pool + `sqlx::migrate!` runner + idempotent `ON CONFLICT`
  writers; `Cursor` is a typed watermark. Its own comment names the
  seam: *"the geyser path would implement the same `poll` shape behind
  the same decode + store seam."* That is the poll source + store sink.
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

**Delivery semantics — at-least-once (store sink).** The cursor is saved
*after* the batch commits. A crash between commit and cursor-save
re-fetches the last window on restart, and the idempotent upsert absorbs
the duplicate. The live sink is best-effort by design — a slow bot
consumer drops to the latest rather than stalling the source (§13).

______________________________________________________________________

## 4. Adapters

Source adapters are the reusable connectors a source composes, each
**feature-gated** so a consumer compiles only the transport it uses.
Adapters **belong in `feeds/`**, so a venue is written **once** and
consumed by collectors and bots alike rather than stranded in whichever
app needed it first. The maker's price sources already sit there; the
Coinbase collector's does not yet, and relocating it is a tracked task
(Status, above).

- **HTTP-REST** (`feature = "http"`, `reqwest` over TLS) — a small JSON
  client: a base URL, a shared client, and `get_json(path, query)`.
  Consumers: the Coinbase reference feed, the FX and issuer-rate feeds,
  and the maker's own price polls.
- **RPC-poll** (`feature = "rpc"`, the solana 3.x client tree) — the
  indexer's `RpcPollSource`, generalized over program id: poll
  signatures newest-first, fetch each transaction at `finalized`,
  flatten inner instructions into ordered, decoded blobs. Consumer: the
  eCLOB indexer.
- **Streaming / WebSocket** (`feature = "stream"`) — a subscribe source
  for the low-latency bot path (a CEX ticker socket, an RPC
  `logsSubscribe`, or geyser). The first concrete adapter is the maker's
  fill socket; a CEX price socket follows when a basis leg needs
  lower latency than its poll cadence gives.

Features are **off by default** so an HTTP-only consumer never compiles
the Solana or streaming trees.

______________________________________________________________________

## 5. Process and deployment model

Two deployment shapes, because the two sink kinds live in different
processes:

- **Store-sink feeds run as their own processes / containers.**
  Separate binaries per feed plus a migrate runner; one versioned Docker
  image builds all of them, and the compose `command` selects the
  process — the same mechanism locally and on the deployed host (§12).
  A run-once migration task precedes one long-lived service per feed
  against the same database. Every feed is idempotent and
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
- **Maker bot (live sink, landed).** The maker-bot's price cascade and
  `logsSubscribe` fill walk run on `feeds`: three HTTP price sources
  (CoinGecko, CoinMarketCap, ECB/Frankfurter) and the fill `logsSubscribe`
  socket — bridged through the stream seam (§4) — fan onto in-process
  forward (live) sinks its synchronous tick loop drains with `try_recv`, on
  a small background runtime. The taker bot has no bespoke price or fill
  feed to migrate: it is a stochastic flow generator sizing orders against
  the live on-chain book, so it stays as the *producer* of the fills the
  maker now consumes.
- **The eCLOB indexer (store sink, migration).** The indexer adopts the
  RPC source + store sink + cursor while keeping its own writers,
  aggregator, and `/v1`. Deferred so the extraction does not destabilize
  a merged component; the crate is designed to fit it.

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

**Table ownership — one writer, unrestricted readers.** Every table has
exactly one writer app. Reads are deliberately unrestricted; that is the
point of sharing an instance.

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

| Role                             | Source                                                                                |
| -------------------------------- | ------------------------------------------------------------------------------------- |
| FX anchor (`fiat/USD`)           | Pyth Hermes FX / OANDA streaming; CME 6E in session; ECB / Frankfurter daily fallback |
| Basis (`token/fiat`, `USDC/USD`) | Coinbase `<token>/USDC`, Binance `EUR/USDT`                                           |
| Peg truth                        | Circle / issuer redemption rate                                                       |
| Token/USD, last resort           | CoinGecko / CoinMarketCap — reflexive, never the anchor                               |
| Macro overlay                    | Econ-calendar loader (ECB / FOMC / CPI / NFP times)                                   |

**Coinbase is the proof feed and the first adapter.** The Exchange
public REST API is keyless and reachable; its candles endpoint returns
`[time, low, high, open, close, volume]` arrays (epoch seconds, at most
300 buckets per request, epoch `start` / `end` accepted), which
backfills and polls cleanly — it validated the whole framework end to
end before any harder source.

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
| Issuer / peg rates     | Order of a day                    | Collector                              |
| Econ calendar          | Order of a day, static download   | Collector                              |
| On-chain (indexer RPC) | Framework poll interval           | Indexer                                |

The maker stays **one multi-market process**, which is what makes
batching natural — a single poll serves every market it quotes. If
per-market maker processes are ever split out they would need a local
price fan-out; that is the trigger to revisit this, not a problem to
pre-solve.

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

______________________________________________________________________

## 12. Deployment — local demo and cloud

**`make demo` flies the full production stack locally** — the production
maker, the collectors, the shared database, the dashboards, and the TUI,
from the same compose file and the same images the cloud host runs. Test
like you fly: there is no demo-only feed tier and no demo-only bot code,
so the thing demoed is the thing deployed.

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
- **Backfill windowing.** The indexer's poll takes the newest batch and
  advances, so a backlog larger than one batch skips the middle
  (`indexer.md` §9). The framework should offer a paged-backfill helper
  so every poll source inherits the fix.
- **Observability hook.** A metrics seam (records/batch, cursor lag,
  error rate) the runner emits, so a deployed feed is observable without
  per-feed wiring. Noted, not built in the first cut.
- **FX bar source.** Pyth Benchmarks vs. a paid FX vendor, and its cost
  and history depth.
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
