<!-- cspell:word backfilling -->

<!-- cspell:word backpressure -->

<!-- cspell:word CoinGecko -->

<!-- cspell:word Fargate -->

<!-- cspell:word reqwest -->

<!-- cspell:word TIMESTAMPTZ -->

<!-- cspell:word upserts -->

# Dropset Data Feeds — Ingestion Framework Design

The **`feeds`** crate: a shared framework for pulling external and
on-chain data from a **source**, decoding it into typed records, and
fanning those records out to one or more **sinks** — a durable Postgres
store *and* live in-process consumers such as the market-making bots.
The same source feeds both: the survey persists it for analysis while a
bot reads the live tail to quote against. This document is the design
**spec** — what to build, what to reuse, and the decisions taken before
code lands.

**Doc boundary.** This is infrastructure shared by several consumers:
the off-chain event indexer ([`indexer.md`](indexer.md)), the
FX-stablecoin liquidity survey ([`fx-survey.md`](fx-survey.md)), and the
maker / taker bots. It defines *how* data is sourced and delivered,
never *what* a consumer does with it — the survey's schema and analyses,
the indexer's `/v1`, and the bots' quoting logic all stay with the
consumers.

**Status.** Greenfield — there is no `feeds/` tree yet. The seams this
crate unifies already exist in the repo (see §2); this spec is the plan
to lift them into one framework and prove it with the survey's first
feed. Building the crate, and migrating each existing consumer onto it,
are separate tracked tasks — the survey is the first consumer.

______________________________________________________________________

## 1. Goals and non-goals

**Goal — one ingestion substrate, many consumers.** Adding a data
source should be: implement a `Source`, pick the sinks, ship it.
Everything else — the drive loop, backoff, cursor persistence, graceful
shutdown, the fan-out — comes from the framework, so neither a new
survey feed nor a bot's price feed re-solves it. Crucially, the *same*
source can drive a durable store and a live consumer at once.

**In scope:**

- A **`Source`**: fetches or subscribes to a data source and yields
  typed records. Two drive shapes — a **poll** source (REST / RPC,
  backfill + interval) and a **subscribe** source (streaming / WebSocket
  push, for bot-latency consumers).
- **`Sink`s** that consume records, fanned out per source:
  - a **store sink** — idempotent Postgres persistence behind a
    resumable cursor (the warehouse path: survey, indexer);
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

- **The consumers' own logic** — the survey's table schemas and SQL, the
  indexer's aggregation and `/v1`, and above all the **bots' quoting /
  trading decisions**. The framework delivers records; a bot decides
  what to do with them.
- **The warehouse destination beyond Postgres.** Promoting Postgres to
  Aurora + an S3 Parquet archival tier is the survey's forward-stack
  task; the store sink targets a `PgPool` and the connection string
  decides local vs. Aurora.

______________________________________________________________________

## 2. What already exists — the reuse surface

The framework is an **extraction and unification**, not a green-field
invention. Three consumers already ingest data their own way; the
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

A feed = a source + its sink set. The **survey** wires its sources to a
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
consumer drops to the latest rather than stalling the source (§7).

______________________________________________________________________

## 4. Adapters

Source adapters are the reusable connectors a source composes, each
**feature-gated** so a consumer compiles only the transport it uses.

- **HTTP-REST** (`feature = "http"`, `reqwest` over TLS) — a small JSON
  client: a base URL, a shared client, and `get_json(path, query)`.
  Consumers: the Coinbase reference feed, and later Circle-rate and
  FX-vendor feeds.
- **RPC-poll** (`feature = "rpc"`, the solana 3.x client tree) — the
  indexer's `RpcPollSource`, generalized over program id: poll
  signatures newest-first, fetch each transaction at `finalized`,
  flatten inner instructions into ordered, decoded blobs. Consumers: the
  eCLOB indexer and the survey's Orca swap feed.
- **Streaming / WebSocket** (`feature = "stream"`) — a subscribe source
  for the low-latency bot path (a CEX ticker socket, an RPC
  `logsSubscribe`, or geyser). Its shape is defined here; the concrete
  adapter is phased in with the first bot consumer (§7), since the
  survey gate is entirely poll-into-store.

Features are **off by default** so an HTTP-only consumer (the whole
survey gate) never compiles the Solana or streaming trees.

______________________________________________________________________

## 5. Process and deployment model

Two deployment shapes, because the two sink kinds live in different
processes:

- **Store-sink feeds run as their own processes / containers.**
  Separate binaries per feed plus a migrate runner; one versioned Docker
  image builds all of them, and the entrypoint (compose `command`
  locally, the ECS task definition in the cloud) selects the process. A
  run-once migration task precedes one long-lived Fargate service per
  feed against the same database. Every feed is idempotent and
  cursor-resumable, so a restarted task just resumes.
- **Live-sink feeds run in the consumer's process.** A bot links
  `feeds`, constructs a source, wires it to a live sink, and reads the
  channel — no separate container, no database. The same adapter code
  serves both shapes; only the sink and the host process differ.

Locally the store-sink shape is a `docker-compose.yml` (Postgres + a
one-shot migrate + one service per feed); that compose file is the
deploy rehearsal the forward-stack task translates to CloudFormation
without changing images or process boundaries.

______________________________________________________________________

## 6. Consumers and boundaries

- **Survey feeds (first consumer).** The FX-stablecoin survey builds its
  sources on this crate into a store sink: an HTTP Coinbase reference
  feed first (the proof feed), then FX / Circle / econ-calendar feeds and
  an Orca swap feed. See [`fx-survey.md`](fx-survey.md).
- **Maker / taker bots (live sink).** A bot's price / fill sources move
  onto `feeds` and read a live sink, replacing the bespoke cascade and
  `logsSubscribe` walk (§2). Migrating them is a follow-up task, folded
  into the bot work; the source/sink split exists so they can.
- **The eCLOB indexer (store sink, migration).** The indexer adopts the
  RPC source + store sink + cursor while keeping its own writers,
  aggregator, and `/v1`. Deferred so the extraction does not destabilize
  a merged component; the crate is designed to fit it.

______________________________________________________________________

## 7. Open questions

- **Live-sink backpressure.** A slow bot consumer must not stall a
  source shared with a store sink. A bounded broadcast channel that
  drops to the latest (a bot wants freshest, not complete) is the likely
  policy — confirmed when the first bot consumer lands.
- **Streaming adapter phasing.** The subscribe source (§4) is a seam
  until a bot needs it; the survey gate does not. Whether the first
  streaming adapter is a CEX socket or an RPC `logsSubscribe` follows the
  first consumer's need.
- **Backfill windowing.** The indexer's poll takes the newest batch and
  advances, so a backlog larger than one batch skips the middle
  (`indexer.md` §9). The framework should offer a paged-backfill helper
  so every poll source inherits the fix.
- **Observability hook.** A metrics seam (records/batch, cursor lag,
  error rate) the runner emits, so a Fargate feed is observable without
  per-feed wiring. Noted, not built in the first cut.
