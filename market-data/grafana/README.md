# Grafana: the market-data ingestion dashboard

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

It serves on **<http://localhost:3200>** and opens on the ingestion
dashboard. Append `?kiosk` to the URL for a chrome-free view — no nav,
no side menu — which is what you want on a screenshare or a screenshot.

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
edit survives until the next `up` and no further, so the committed JSON
and what you are looking at cannot quietly diverge for long.

## Changing a dashboard

Two loops, both live. The provider re-reads every 30s, so **editing the
committed JSON shows up in the browser within half a minute** — no
restart, no rebuild.

For anything visual, go the other way round: tweak the panel in the
browser until it reads well, then **export and commit**. Anonymous
access is `Editor` for exactly this reason, so:

1. Edit the panel, `Save` (it lands in Grafana's ephemeral store).
1. `Export` → `Export as JSON` → save over the file in `dashboards/`.
1. Commit it. If you skip this step the next `up` discards the change,
   which is the intended failure direction.

## Reading the dashboard

Top-down: the two stat rows answer *is ingestion alive right now*, and
everything below answers *is it any good*.

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
EC2 compute box of `docs/data-feeds.md` §12, where only the host and the
credentials change.

Two things a real deployment needs that are **not** solved here, since
this stack is loopback-only:

- **A password that is not in the repo.** The `dropset_ro` password is a
  throwaway matching the hardcoded superuser password beside it in
  `infra/localnet/docker-compose.yml`. Rotate it out of band
  (`ALTER ROLE dropset_ro PASSWORD …` from the secret store), not by
  editing the migration — migrations are additive-only and that one has
  already been applied, so an edit there only changes what a *fresh*
  database gets.
- **Authentication in front of Grafana.** Anonymous `Editor` is right
  for a loopback dev tool and wrong for anything reachable.
