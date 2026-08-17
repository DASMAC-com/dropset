# Grafana: the market-data dashboard

Data observability over the shared `dropset` database
(`docs/data-feeds.md` §8). This directory is **configuration only** —
there is no producer code here. The collectors already write their
tables; this is the operator's view of what they are producing, so a
feeds change can be verified by looking at it the way the TUI verifies
the maker.

```sh
make grafana        # Grafana alone, against whatever history is on the volume
make collectors-up  # the Coinbase feed + Grafana together
make grafana-down   # stop it; leaves postgres and the data alone
```

It serves on **<http://localhost:3200>** and opens on the `market-data`
dashboard. Append `?kiosk` to the URL for a chrome-free view — no nav,
no side menu — which is what you want on a screenshare or a screenshot.

The dashboard is deliberately named just `market-data`, not something
narrower: its panels are ingestion-focused, and maker telemetry lands in
this same tree later.

A second dashboard, **FX analytics** (`fx-analytics`), now sits beside
it. The split is by question rather than by subject: `market-data`
answers *is ingestion alive and any good*, and `FX analytics` answers
*what does the collected history say*. Keeping them apart matters for
the ingestion view in particular, whose value is that a wall of green
means a feeds change works; mixing regime analytics into it would
dilute that read.

`FX analytics` is ordered **raw first**. The top row is the plain price
series for one selected venue and product, beside a table of everything
the store holds — per source and product, its bar count, newest bar, and
age. Those answer *is there data and does it make sense* before
anything derived is shown, and the table is a table rather than a tile
per feed precisely because the roster grows. Below them sit the three
regime panels: realized volatility by hour, by FX trading session, and
weekend against weekday.

It deliberately reads **one product at a time** rather than overlaying
the roster. Selecting a currency and getting every pair that involves it
is a better interface and is tracked separately; it needs products
tagged by currency family, which is a schema change rather than a
dashboard one.

The **basis against an FX anchor** is not a panel in this pass. Its
query is committed and works — see `market-data/analytics/` — but a
two-leg panel is only meaningful when both legs are the same currency
family, and nothing in the dashboard enforces that yet. An anchor picker
that silently falls back to another currency's rate renders a large,
stable, entirely meaningless number instead of failing visibly, which is
worse than showing nothing.

## The tree

| Path                        | What it is                                |
| --------------------------- | ----------------------------------------- |
| `provisioning/datasources/` | The shared Postgres, as a read-only login |
| `provisioning/dashboards/`  | The loader that points at `dashboards/`   |
| `dashboards/`               | The dashboards themselves, as JSON        |

All three are bind-mounted **read-only** into the container, so the repo
is the source of truth and the container cannot edit its way out of it.

Nothing here is Grafana-instance state: the `grafana` service has **no
volume**. That is what keeps the arrangement honest — an uncommitted UI
edit lives in the container's writable layer and dies with the
container, so the committed JSON and what you are looking at cannot
quietly diverge for long.

Be precise about when that happens, because the safety net is narrower
than "restart it": a plain `make grafana` re-runs `up -d`, which leaves
an already-running container **alone**, so it discards nothing. What
clears the edit is **recreating** the container — `make grafana-down`
(`rm -sf`) then `make grafana`, or `make clean-docker`.

## Changing a dashboard

Two loops, both live. The provider re-reads every 30s, so **editing the
committed JSON shows up in the browser within half a minute** — no
restart, no rebuild.

For anything visual, go the other way round: tweak the panel in the
browser until it reads well, then **export and commit**. Anonymous
access is `Editor` for exactly this reason, so:

1. Edit the panel, `Save` (it lands in Grafana's ephemeral store).
1. `Export` → `Export as JSON` → save over the file in `dashboards/`.
1. **Check the template variables** — see the trap below.
1. Commit it. If you skip this step the change dies the next time the
   container is recreated (`make grafana-down && make grafana`), which
   is the intended failure direction.

### The export trap: variable queries must stay plain strings

A query-backed template variable has to carry its SQL as a **plain
string**:

```json
"query": "SELECT DISTINCT source FROM cex_prices ORDER BY 1"
```

Grafana's UI exports it as an **object** instead
(`{"rawSql": …, "refId": …, …}`), and in that form the variable query is
**never executed at all** — verified against the Postgres statement log,
holding `refresh` constant, with and without the object. So after any UI
export, convert each `templating.list[].query` back to a string.

The failure is nasty because it is quiet. **What you will see today** is
the dropdowns never populating, while the panels keep painting normally
— because the two multi-selects fall back to their literal `.*`
allValue, which needs no query. So the dashboard looks fine and has
merely lost its filters. Check the dropdowns, not the panels.

Historically it was louder and more confusing: before the panels used
the regex form below, an unresolved variable produced `IN ()` and every
panel failed with `syntax error at or near ")"`, which reads as broken
panel SQL rather than as a variable that never resolved. That symptom
can no longer occur, and the safeguards below are why.

Two deliberate safeguards are already in place, and are worth keeping:

- Each multi-select variable sets `allValue` to the literal `.*` and the
  panels match with an anchored regex (`source ~ '^${source:regex}$'`)
  rather than `IN (…)`. An `IN ()` on an empty variable is a syntax
  error; the regex form degrades to "match everything" and keeps
  painting. This is what kept the panels alive while the bug above was
  being tracked down.
- `candle_source` commits a concrete default, so the candlestick paints
  on first load without waiting on its query.

## Reading the dashboard

Top-down: the two stat tiles across the top answer *is ingestion alive
right now*, and everything below answers *is it any good*.

The two freshness tiles look redundant and are not, which matters most
on first contact:

- **Feed cursor age** is wall-clock, from `feed_cursors.updated_at` —
  the only true liveness signal in the schema. It says the process is
  running and committing.
- **Last candle age** is data recency, from `max(bucket_start)`. Its
  floor is one granularity, because the collector persists only
  *closed* buckets: a perfectly healthy 60s feed reads 60–120s and
  never 0. The thresholds are set above that floor accordingly.

**During a backfill they deliberately disagree**, and that is the single
most confusing state to walk into. A cold collector starts 60 days back
and works forward, so cursor age reads green (it is very much alive)
while candle age reads red (the newest bucket is weeks old) and the
throughput panel is empty (no buckets land in the last 15 minutes).
Nothing is broken; give it a few minutes and all three converge.

Two other honest limits worth knowing before you trust a panel:

- **Throughput is measured in candle-bucket time, not insert time.**
  `cex_prices` records no insert timestamp, so "rows per minute" counts
  buckets by the minute they *cover*. For a live 60s feed that is a flat
  line at one row per product per minute. A collector that dies
  mid-window visibly drops to zero; one that was already dead for the
  whole window contributes no series at all rather than a zero line —
  which is what the freshness tiles are for.
- **The candlestick panel reads one source at a time.** Candles from two
  venues share a bucket key but are different series, so overlaying them
  would interleave bars rather than compare them. Compare sources on the
  overlay panel instead; the `Candle source` variable picks which venue
  gets bars.

The multi-source panels are written source-generic — grouped by
`source`, driven by template variables — so a second collector appears
in them with no dashboard edit. With one feed running they are a single
series, which is correct, not a bug.

## Deploying it elsewhere

Every value that differs between this compose stack and a cloud host is
an environment variable (`DROPSET_DB_HOST`, `DROPSET_DB_NAME`,
`DROPSET_DB_USER`, `DROPSET_DB_PASSWORD`, `DROPSET_DB_SSLMODE`), with
the local defaults set on the `grafana` service rather than in the
provisioning files — Grafana expands `$VAR` in those files but has no
default syntax of its own. So the identical tree ships unchanged to the
EC2 compute box of `docs/data-feeds.md` §12, where only those five
values change — the host, the credentials, and `DROPSET_DB_SSLMODE`,
which defaults to `disable` and has to become `require` the moment the
database stops sitting on loopback beside it.

Two things a real deployment needs that are **not** solved here, since
this stack is loopback-only:

- **A password that is not in the repo.** The `dropset_ro` password is a
  throwaway, like the hardcoded superuser password beside it in
  `infra/localnet/docker-compose.yml`. Note the compose default is a
  **soft** one (`${DROPSET_DB_PASSWORD:-dropset_ro}`), so a host that
  forgets to export the variable silently falls back to the committed
  throwaway rather than failing to start — on a real deployment, set it
  explicitly. Rotate it out of band
  (`ALTER ROLE dropset_ro PASSWORD …` from the secret store), not by
  editing the migration — migrations are additive-only and that one has
  already been applied, so an edit there only changes what a *fresh*
  database gets.
- **Authentication in front of Grafana.** Anonymous `Editor` is right
  for a loopback dev tool and wrong for anything reachable. Three
  settings have to travel together for that to stay true —
  `GF_AUTH_ANONYMOUS_ENABLED`, `GF_AUTH_BASIC_ENABLED: 'false'`, and
  `GF_AUTH_DISABLE_LOGIN_FORM: 'true'` — because `admin`'s default
  password is unset and is unreachable only while the latter two hold.
  An anonymous Editor can also author alert rules and contact points,
  so read *egress* is bounded by the loopback bind, not by the
  read-only role.
