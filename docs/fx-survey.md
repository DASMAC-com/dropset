<!-- cspell:word arbed -->

<!-- cspell:word backfill -->

<!-- cspell:word Bitquery -->

<!-- cspell:word capturability -->

<!-- cspell:word capturable -->

<!-- cspell:word CoinGecko -->

<!-- cspell:word Coinbase -->

<!-- cspell:word EURC -->

<!-- cspell:word Fargate -->

<!-- cspell:word Flipside -->

<!-- cspell:word FOMC -->

<!-- cspell:word Orca -->

<!-- cspell:word Parquet -->

<!-- cspell:word Pyth -->

<!-- cspell:word sqrt -->

<!-- cspell:word stablecoin -->

<!-- cspell:word stationarity -->

<!-- cspell:word USDC -->

<!-- cspell:word Whirlpool -->

# FX-Stablecoin Liquidity Survey — Plan and Data Sources

Read-only reconnaissance and analytics on onchain FX-stablecoin pools —
proving or killing the **under-arbed thesis** and mapping the
opportunity by regime. The tool is currency-agnostic; its first and
flagship target is the **Orca EURC/USDC Whirlpool**, and the same feeds,
schema, and analyses extend to the other FX-stablecoin pairs the product
covers. No capital and no transaction landing — the only external
dependency is read access to market data. The deliverable is a chart
report that doubles as pitch material for aggregator routing and the
Circle LP conversation, so it is doubly load-bearing.

**Doc boundary.** This is the survey's plan of record: its milestones,
data sources, storage schema, analyses, and work breakdown. It consumes
the ingestion framework in [`data-feeds.md`](data-feeds.md) — every data
source below is a *source* in that framework, wired to a store sink —
and it does not re-specify how ingestion works. Those same feeds also
back the maker / taker bots through a live sink; the survey is just the
first (durable) consumer. The fair-value model it prices against (FX ×
basis, §4) is the pricing thesis that inverts the CoinGecko-primary
approach sketched in [`market-making-mvp.md`](market-making-mvp.md) §1.

**Status.** Greenfield — no `analytics/fx-survey/` tree yet. This doc is
the plan; the implementation is a tree of tracked tasks (§7). This
worktree ships **only this plan** (this doc, `data-feeds.md`, and the
task breakdown); every code unit is a separate task.

______________________________________________________________________

## 1. Goals and non-goals

**Goal — a go/no-go on the maker, backed by numbers.** Quantify how
dislocated a target pool is from fair value, how long dislocations
persist, who (if anyone) closes them, and whether the edge is
maker-capturable or a taker-only race. Produce the money chart and the
stats that make the decision, and that pitch aggregator routing and
Circle. Run it first on EURC/USDC; keep every step parameterized by pair
so a second currency is a config change, not a rewrite.

**In scope:** the local gate (§2 milestone 1) end to end — a Postgres
store, a bounded historical backfill of the deepest target pool plus the
reference series, and every analysis in §6, culminating in the go/no-go
decision.

**Non-goals:** any capital or transaction landing (this is read-only);
the maker and taker themselves (separate initiative tasks); and any
always-on cloud infrastructure until the gate says go. The forward
collection stack (§2 milestone 3+) is authored only if the gate passes.

______________________________________________________________________

## 2. Prototype-first milestones

The thesis is retrospective and slow-moving, so it is provable on a
bounded historical backfill with **no always-on infrastructure**. Build
the local gate first; stand up the cloud stack only if the gate says go.

1. **Gate (local, no cloud).** Postgres in Docker + a ~60–90 day
   backfill of the deepest EURC/USDC pool and the reference series +
   every analysis in §6 (including the basis-process characterization
   and the maker-vs-taker capturability split) → prove or kill the
   thesis.
1. **Decision.** Go/no-go on the maker. If go, promote to forward
   collection (the taker also needs live data).
1. **Forward stack (cloud, only after the gate passes).** The *same*
   Postgres schema on Aurora Serverless v2 (idle-heavy load); long-lived
   collectors on ECS Fargate (persistent connections, not Lambda); S3
   Parquet as the archival raw tier. Authored as CloudFormation on the
   existing AWS foundation.
1. **Forward collectors** capturing to Postgres (+ S3 Parquet); derived
   tables refresh.
1. **Chart report** — the money chart and overlays; the
   aggregator-routing and Circle-LP pitch material.

Only milestone 1 is on this initiative's critical path for the maker
decision; it needs no cloud foundation and is the first thing built.

______________________________________________________________________

## 3. Ingestion — sources on the shared framework

Every data source is a `Source` on the [`data-feeds.md`](data-feeds.md)
framework, wired to a store sink: a migrate-runner process plus one
process per feed, one versioned image, run locally under
`docker-compose` and (post-gate) as Fargate services against Aurora.
Because the store sink targets a `PgPool`, the gate schema lifts to
Aurora unchanged — one engine, one `sqlx` / migration toolchain (shared
with the indexer), no dialect port. The same sources later back the bots
through a live sink (`data-feeds.md` §6).

The feeds:

- **Coinbase reference feed** — HTTP adapter; the proof feed (§4), built
  first.
- **Orca swap + pool-state feed** — the RPC adapter, or a decoded-data
  provider (decision pending, §4).
- **Reference-data feeds** — FX EUR/USD, the Circle rate, and an
  econ-calendar loader (HTTP / static).

______________________________________________________________________

## 4. Data sources and decisions

Fair value is **`FX_EUR/USD × basis`**, where
`basis = (EURC/EUR) ÷ (USDC/USD)` — a two-peg model, **not** a CoinGecko
token price (that is reflexive against the very pool being measured). No
market-data credentials exist in the repo or environment today; each
source below is either keyless or its own provisioning task.

- **Coinbase EURC/USDC (chosen — the proof feed).** The Exchange public
  REST API is keyless and reachable; both `EURC-USDC` and `EURC-USD`
  are `online` and flagged `fx_stablecoin`. The candles endpoint returns
  `[time, low, high, open, close, volume]` arrays (epoch seconds, ≤ 300
  buckets per request, epoch `start` / `end` accepted), which backfills
  and polls cleanly. This is the gate's CEX reference price and the
  lead-lag / observability input, and it validates the whole framework
  end to end before any harder source.
- **Orca EURC/USDC Whirlpool swaps + pool state (decision pending).**
  The dislocation series needs decoded swaps and the pool `sqrt_price`
  series. Two paths: a **decoded-data provider** (Dune / Bitquery /
  SQD / Flipside — closest to the SQL-analyses model, may need a key) or
  **archival-RPC decode** via the framework's RPC adapter (no external
  data dependency, more work). Choosing between them is its own task;
  it is the one hard ingestion decision and is deliberately deferred out
  of the gate's first feed.
- **FX EUR/USD** — minute bars from Pyth Benchmarks or an FX vendor.
- **Circle rate** — the EURC/USDC redemption anchor that enforces basis
  reversion (Circle-Mint arbitrage).
- **Econ calendar** — ECB / FOMC / CPI / NFP event times, a static
  download, for the macro-event regime overlay.

______________________________________________________________________

## 5. Storage schema

Postgres from the start so the local gate and the cloud forward stack
share one engine, schema, and migration toolchain, and the gate schema
lifts to Aurora unchanged. Every table is keyed by pair so a second
currency is additive. The tables, each owned by the source or analysis
that fills it:

- `cex_prices` — CEX reference candles (the Coinbase feed).
- `swaps`, `pool_snapshots` — decoded Orca swaps and the pool
  `sqrt_price` series.
- `fx_ticks`, `basis_series` — FX EUR/USD and the derived basis series.
- `fx_events` — the economic-calendar overlay.
- `dislocations`, `swap_impact`, `regimes` — the derived analysis
  tables (the money-chart series, per-swap impact decomposition, and the
  regime tags stats are sliced by).

______________________________________________________________________

## 6. Analyses

All run in SQL over Postgres. Each names the tables it reads / writes.

- **Baseline activity** — hourly swap volume, count, and trade-size
  distribution on the deepest target pool, benchmarked against a
  SOL/USDC pool to quantify "low volume vs. meme coins."
- **Dislocation time series (the money chart)** — Orca mid vs. the
  FX × basis fair value, the gap shaded.
- **Dislocation stats** — `|gap|` distribution (bps), half-life (slots
  until back within X bps of fair), and the exploitable-window duty
  cycle (fraction of time beyond fee + cost).
- **Who closes the gap** — the first swap that pushes the pool back
  toward fair: latency from the FX move, signer / program (Jupiter /
  direct / searcher), priority fee. Slow + cheap + organic implies
  under-arbed.
- **Counterfactual PnL** — a taker hitting every gap over cost;
  cumulative gross / net, split into captured-by-others vs.
  decayed-uncaptured (the second bucket is the edge).
- **Lead-lag** — cross-correlation of FX spot vs. Coinbase EURC/USDC
  vs. other venues vs. the Circle rate against Orca; settles which
  signal leads.
- **Flow regimes** — tag every window by FX session (the London–NY
  overlap is peak), the weekend / overnight gap (FX thin or closed while
  Orca drifts on crypto — the Sunday FX-reopen gap is likely the largest
  repeatable dislocation), scheduled macro events, post-large-swap
  windows, and depeg / stress episodes (a USDC or token depeg, or a
  redemption-arb suspension like the SVB weekend — the regime where the
  basis stops mean-reverting and the maker is most exposed); slice all
  stats by regime.
- **Large-swap classification** — direction vs. fair (toward = toxic
  arb, away = liquidity), a permanent-vs-temporary impact decomposition
  (snap-back = benign, sticky = informed / toxic), by signer / routing /
  time-of-day.
- **Missing-hedge-venue landscape** — log the absence of an EURC borrow
  market and of an EURC route / perp (ecosystem-maturity evidence).
- **Basis-process characterization (the maker go/no-go).** Treat the
  basis as its own series: mean, volatility, half-life of reversion,
  stationarity (whether it mean-reverts or random-walks), and jump / tail
  frequency, **per market**. Whether the basis mean-reverts is the
  assumption the whole maker thesis rests on — this block proves or
  kills it.
- **Observability** — two questions. Survey-time: when FX / basis moves,
  is it visible on Coinbase before Orca reprints? The single most
  important number for whether the edge is maker-capturable or a latency
  race. Production: define the **realized-vs-modeled basis error** — the
  gap between the live basis and the engine's EMA estimate of it — as the
  standing signal the deployed maker is monitored on, so it runs against
  this characterization rather than blind. The survey sets its healthy
  band and alert threshold.
- **Maker-vs-taker capturability by regime** — does each dislocation
  revert intra-window (maker-capturable) or only at a discrete event
  like the Sunday FX reopen (taker-only)? Attribute the edge across the
  regimes above — calm, weekend / session role-flip, macro-vol spike, and
  depeg / stress — since the split flips by regime: what a maker captures
  in calm is a taker-only gap at the reopen and pure risk under a depeg.
  Distinguishes a maker-viable edge from a taker-only one.

______________________________________________________________________

## 7. Work breakdown

This worktree ships the plan (this doc + `data-feeds.md`). Every code
unit is a separate task, filed as a sibling under the same initiative —
not a subtask of this plan. The framework and the survey app are the
foundation the rest depends on.

| Task                             | Scope                                                                     | Touches                  |
| -------------------------------- | ------------------------------------------------------------------------- | ------------------------ |
| **Survey plan** (this)           | This doc + `data-feeds.md` + the breakdown                                | `docs/`                  |
| `feeds` framework                | The shared crate: source / sink / runner / cursor store / adapters        | `feeds/**`               |
| fx-survey app + Coinbase feed    | The app crate: migrate + Coinbase feed, schema, Docker, the local gate    | `analytics/fx-survey/**` |
| Orca swap + pool-state feed      | Decoded swaps + `sqrt_price`; incl. the provider decision (§4)            | `analytics/fx-survey/**` |
| Reference-data feeds             | FX EUR/USD + Circle rate + basis derivation + econ-calendar loader        | `analytics/fx-survey/**` |
| Dislocation analyses + PnL       | The money chart, gap stats, who-closes-the-gap, counterfactual PnL        | `analytics/fx-survey/**` |
| Maker go/no-go analyses          | Basis-process + capturability + lead-lag + observability                  | `analytics/fx-survey/**` |
| Descriptive analyses             | Baseline, large-swap toxicity, regime tagging, hedge-venue landscape      | `analytics/fx-survey/**` |
| Chart report                     | The money chart + overlays; the aggregator-routing + Circle-LP pitch      | `analytics/fx-survey/**` |
| Forward stack (cloud)            | Aurora Serverless v2 + Fargate collectors + S3 Parquet, as CloudFormation | `infra/aws/fx-survey/**` |
| Bots consume `feeds` (live sink) | Move the bot price / fill sources onto `feeds`; folds into the bot work   | `bots/**`, `feeds/**`    |
| Indexer → `feeds` migration      | Port the eCLOB indexer onto the shared crate                              | `indexer/**`, `feeds/**` |

Dependencies: the `feeds` framework blocks the survey app; the app
blocks every feed; the feeds block the analyses; the analyses block the
report; the forward stack is blocked by the AWS foundation and the gate
decision. The framework blocks the indexer migration and the bot
migration too. These are tracked as native blocking edges plus
file-overlap edges on the shared crate.

______________________________________________________________________

## 8. Open questions

- **Orca data source** — provider vs. archival-RPC decode (§4); the one
  hard ingestion decision, its own task.
- **Backfill window** — 60 vs. 90 days; long enough to span the weekend
  and macro-event regimes with enough repeats to be significant.
- **FX minute-bar source** — Pyth Benchmarks vs. a paid FX vendor, and
  its cost / history depth.
- **Econ-calendar source** — which static feed for ECB / FOMC / CPI /
  NFP times.
- **The load-bearing assumption** — the whole maker thesis rests on the
  basis mean-reverting (§6). EURC has Circle-Mint redemption arbitrage
  enforcing reversion; thinner exotics may not. The basis-process block
  is what turns that assumption into a measured fact.
