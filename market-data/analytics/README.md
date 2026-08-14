# Market-data analytics

Repeatable, committed queries over the shared `dropset` database. These
are the analysis counterpart to `market-data/grafana/`: the dashboards
answer *is ingestion alive and any good*, these answer *what does the
data actually say*.

Everything here is **read-only SQL**. There is no producer code, no
migration, and no view — the queries run against the tables the
collectors already write, so adding or changing an analysis costs a file
and nothing else.

## Not the same thing as `market-data/queries/`

`market-data/queries/` holds SQL the **crate** embeds at compile time
(`include_str!`, see `src/store.rs`). Nothing in this directory is
compiled into anything. Keep the two separate: a file here is run by a
person or by Grafana, never by the collector.

## Running one

Each query takes psql variables and is invoked with `-f`:

```sh
psql "$DROPSET_DB_URL" \
  -v source=coinbase -v product_id=EURC-USDC -v granularity_secs=60 \
  -f market-data/analytics/weekend_vs_weekday.sql
```

Against the compose stack, the database is reachable through the
container:

```sh
docker exec dropset-localnet-postgres-1 psql -U dropset -d dropset \
  -v source=coinbase -v product_id=EURC-USDC -v granularity_secs=60 \
  -f /tmp/analytics/weekend_vs_weekday.sql
```

Bring the data up first with `make collectors-up`. A cold collector
backfills 60 days, so the history is there within a few minutes of first
start rather than accumulating over calendar time.

## The queries

| File                        | Legs | Answers                                                         |
| --------------------------- | ---- | --------------------------------------------------------------- |
| `weekend_vs_weekday.sql`    | one  | Does the product behave differently while interbank FX is shut? |
| `session_windows.sql`       | one  | Does it inherit the Sydney / Tokyo / London / New York rhythm?  |
| `realized_vol_by_hour.sql`  | one  | Sigma by hour and regime — the vol-ladder's input.              |
| `deviation_from_anchor.sql` | two  | The basis: venue price vs. its FX anchor, in bps.               |

Three of the four are **single-leg** — they need only the venue's own
candles. That is why they produce results today while the FX anchor feed
is still being built: the anchor is required for the basis series and
for nothing else.

## Two invariants every query here holds

**Adjacent-bucket returns only.** A log return is computed between two
buckets only when they are genuinely consecutive at the stated
granularity. Skipping this is the single easiest way to produce a
confident wrong answer: a venue emits no candle for a minute with no
trades (~12% of bars on the 60-day EURC-USDC backfill), and interbank FX
leaves a ~48-hour hole every weekend. A plain window function bridges
those gaps and reports one enormous pseudo-return, attributed to
whichever bucket happens to sit on the far side.

**Local wall clock, never fixed UTC offsets.** Session windows and the
FX week boundary are expressed in the relevant city's own timezone and
resolved by Postgres per timestamp. Daylight saving moves these
boundaries twice a year, and the southern hemisphere moves opposite to
the northern — so a hardcoded UTC hour is wrong for a large fraction of
any multi-month window, which is exactly the window these run over.

## Which pair leads

**EUR/USD leads the analytics and the exportable chart.** The AUD pair
is carried as a case study rather than the headline, and the reason is
measured rather than editorial. Over the same 60-day window and venue,
at 60s granularity:

| Product     | Bars   | Bars/day | Thinnest day |
| ----------- | ------ | -------- | ------------ |
| `EURC-USDC` | 71,469 | 1,171.6  | 21           |
| `AUDD-USDC` | 845    | 14.1     | 4            |

`AUDD-USDC` prints in roughly 1% of minutes. That is enough for a
daily-resolution basis level and not enough for an intraday session or
volatility read — a Saturday there produced 8 bars with a 113 bps range
between them. The queries are all product-generic, so nothing needs
rewriting if that book deepens; point them at a different
`product_id`.

The thinness is itself a finding worth keeping rather than a gap to
apologize for: a barely-traded centralized book is the clearest argument
for an on-chain FX market in that currency.

## What the data says so far

Recorded here because it is the shape of the result, not the result
itself — the narrative write-up and the exported image are a separate
deliverable. Measured on `coinbase` / `EURC-USDC` / 60s over
2026-06-15 → 2026-08-14:

- **Weekends are calmer, and their tail is much calmer.** 0.98 bps/bar
  realized vol against 1.42 on weekdays, with the largest single-bar
  move falling from 82 bps to 13 bps.
- **The session rhythm survives into the stablecoin.** London 1.58 and
  New York 1.57 bps/bar against Sydney 1.05 and Tokyo 1.13. A euro
  stablecoin on a crypto venue is most volatile during the euro's home
  session, not on a flat 24/7 profile.
- **The intraday profile has FX's fingerprints on it.** Weekday vol
  peaks at 13:00 UTC (2.65 bps/bar, and the 82 bps outlier), the
  London/New York overlap and the usual US release slot, with a second
  spike at 21:00 UTC around the FX daily roll. Neither peak appears on
  weekends.

## A hazard when the FX anchor lands

Some FX vendors publish a **complete weekend series** — a full 1440-bar
Saturday — while others correctly return nothing, because interbank FX
is closed. Both conventions will exist in `cex_prices` under different
`source` values.

The consequence is narrow and worth stating exactly:

- **Do not** compute a weekend volatility figure from a source whose
  market was closed. It yields a plausible ultra-low sigma for a session
  that never traded, and it fails quietly — it neither errors nor reads
  as zero. Prefer a source whose weekend bar count is zero; absence is
  the honest signal.
- **Do** still compute the weekend deviation series. An anchor holding
  flat while the venue leg keeps moving is the mechanism this directory
  exists to measure: with no arbitrage channel open, the basis is free
  to widen, and that widening is the finding.

Resist the tempting shortcut of detecting such a series by a collapse in
distinct closing prices. It was tested against this repo's own
genuinely-traded tape and it is wrong — a real Saturday carries only
3–15 distinct closes across ~1,000 bars, and across the full window
weekends show *more* distinct closes per bar than weekdays (8.9 vs 5.5
per 1,000). Sparse pricing is the normal appearance of a quiet real
market, so anything keyed on that ratio fires on honest data.
