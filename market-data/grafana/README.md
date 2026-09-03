# Grafana: the Dropset dashboards

Observability over the shared `dropset` database (`docs/data-feeds.md`
§8). This directory is **configuration only** — there is no producer
code here. The collectors and the maker bot already write their tables;
this is the operator's view of what they are producing, so a feeds
change can be verified by looking at it.

It is also the **read surface** in the ratified split: Grafana owns time
series, statuses, and alerting, and the TUI owns write commands plus only
the state displays needed to command safely. So there is no feed-health
pane in the TUI, deliberately — history and alerting live here, where
they still work when nobody is watching a terminal.

```sh
make grafana        # Grafana alone, against whatever history is on the volume
make collectors-up  # every collector + Grafana together
make grafana-down   # stop it; leaves postgres and the data alone
```

It serves on **<http://localhost:3200>** and opens on the `market-data`
dashboard. Append `?kiosk` to the URL for a chrome-free view — no nav,
no side menu — which is what you want on a screenshare or a screenshot.

The dashboard is deliberately named just `market-data`, not something
narrower: its panels are ingestion-focused, and maker telemetry now
lands in this same tree — see **Maker operations** below.

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

| Path                        | What it is                                  |
| --------------------------- | ------------------------------------------- |
| `provisioning/datasources/` | The shared Postgres, as a read-only login   |
| `provisioning/dashboards/`  | The loader that points at `dashboards/`     |
| `provisioning/alerting/`    | Alert rules, as YAML                        |
| `dashboards/`               | The dashboards themselves, as JSON          |
| `sql/`                      | Generated mirror of every query — see below |

Those **first four** are bind-mounted **read-only** into the container,
so the repo is the source of truth and the container cannot edit its way
out of it. `sql/` is generated and is not mounted at all; nothing reads
it at runtime.

Each of the three `provisioning/` subdirectories is mounted by name,
never the `provisioning/` parent — and that is load-bearing rather than
tidy. The image ships its own `plugins/` and `alerting/` provisioning
directories, so mounting the parent shadows them; before `alerting/`
existed here that cost two `level=error` lines on every boot, and now it
would shadow the very directory being provisioned, which is the one
mistake that would still look like it worked.

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

### The SQL mirror, and why a dashboard edit is a two-file commit

Every query in this tree — panel, annotation, template variable, **and
alert rule** — is mirrored into its own `.sql` file under `sql/` by
`.claude/tools/dashboard_sql.py`. The JSON and the YAML stay the source
of truth; nothing writes back to them.

Regenerate with `make dashboard-sql` and **commit the result in the same
commit as the change**. A pre-commit hook refuses a stale mirror, so this
is not optional — and it fires on deletes too, which is why it is an
`always_run` hook rather than a `files`-scoped one.

It earns the extra file three ways: a one-character edit inside a
1,500-character JSON string becomes a one-line diff; `make lint` runs
sqlfluff over every query as Postgres, the only mechanical check this SQL
has; and three silent-in-Grafana failure modes are refused outright — a
nested paren in a macro argument, which Grafana truncates; a
`:regex`-formatted variable inside a quoted literal, which does not
escape quotes; and **any unformatted variable inside a quoted literal**,
which carries the same exposure for the same reason.

That third guard is why a template variable is written
`${name:sqlstring}` with **no surrounding quotes** rather than
`'$name'` — the formatter quotes and escapes the value itself. For a
multi-select use `= ANY (ARRAY[${name:sqlstring}]::text[])`, which also
stays valid SQL when nothing is selected, where `IN ()` is a syntax
error. A variable reaching an integer column takes
`${name:sqlstring}::bigint`: the cast keeps the type and turns an
injected value into a cast error rather than a predicate.

**The alert rules are covered, and the reader is deliberately not
PyYAML.** The `dashboard-sql-lint` hook runs in an environment holding
only sqlfluff, so importing a YAML library on that shared path would
break that hook while leaving the mirror check green — a failure in the
gate least related to the change. The tool instead implements YAML's
block-scalar rule directly to lift `rawSql`, which is a structural read
rather than a regex sweep. What keeps it honest is a test, not a
promise: the suite runs under the ambient interpreter, where PyYAML *is*
available, and asserts the two agree byte-for-byte on the committed
files. A rule using YAML the subset reader cannot handle fails that test
rather than silently mirroring the wrong text. A folded (`>`) `rawSql`
is refused outright, since folding rewrites newlines.

Mirror filenames are keyed on the **rule `uid`** for alerts, which is a
stable handle: a rule can be retitled or reordered and its mirror file
stays put. Dashboard panels are **not** the same — their filename
carries the panel `id` *and* a slug of the title, so **retitling a panel
renames its mirror file**. That is why `extract` prunes orphans: a
renamed panel would otherwise leave its old mirror behind to be linted
forever, reading as a query that still exists. A duplicate path is
refused rather than assumed away.

**Do not "fix" what sqlfluff would flag in the quoted aliases.** Grafana
binds panels by name — `AS "time"`, and the `"open"`/`"high"`/`"low"`/
`"close"` of the candlestick panels — so renaming one silently empties
the panel. `cfg/sqlfluff-dashboards.cfg` narrows the rule set for
exactly this reason; widening it needs browser verification afterwards.

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

Top-down: the two fair-price panels at the top answer *what does the
system believe, and why*, the two stat tiles below them answer *is
ingestion alive right now*, and everything after that answers *is it any
good*.

### The two fair-price panels

These are the estimator's output (`docs/market-making.md` §1
"Fair-price estimation"), and they read together.

- **Fair price over its sources** draws three lines per market — the
  composed fair price, the FX leg's fused estimate, and the FX leg's fast
  consensus median — over every selected source's raw ticks. **Read the
  gaps**, which is why all three are drawn rather than just the answer.
  Fused-vs-fast is the estimator's whole contribution: the two sit on top
  of each other while the sources agree and separate exactly when a
  source is being ignored or a dislocation is being adopted.
  Fused-vs-scatter is the estimator declining to chase a stray print.
- **Fusion weight by source** explains the first panel: when the fused
  line stops tracking a source, this says by how much and from when.

Two things to know before reading either:

- **A weight series pinned at zero is the interesting case**, not an
  empty one. Zero means the source answered and was **trimmed** — it sat
  outside the dispersion band of the fast consensus, so the estimator
  declined to believe it. That is a sick feed, a mis-mapped product, or a
  real disagreement between an official reference rate and the tape. A
  source that simply stopped answering has no row at all and leaves a
  gap instead, so the two states are distinguishable on sight.
- **The Market picker and the Product/Source pickers are independent.**
  A market is keyed by token symbol (`EURC`), a feed product by pair id
  (`EUR-USD`), and the schema holds no mapping between the two
  vocabularies — so pairing them is yours to do. A fused line drawn over
  an empty scatter means the pickers disagree, not that data is missing.

The Market variable reads `maker_legs`, so on a database no maker has run
against it is empty. **Fusion weight by source** then goes blank; **Fair price
over its sources** does *not* — its raw-tick arm carries no market predicate, so
it still draws the full source scatter with no fused line over it. That is the
same picture a picker mismatch produces, and the two are told apart by whether
the Market picker has any values to offer at all. Either way it is isolated:
the collectors and the maker are independent writers, and every ingestion panel
below is unaffected.

### The two freshness tiles

They look redundant and are not, which matters most on first contact:

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

## Maker operations

The third dashboard, **Maker operations** (`maker-operations`), reads the
maker bot's own telemetry rather than the collectors' — the tables
`db-schema/migrations/0003_maker_telemetry.sql` creates. The spec is
`docs/market-making.md` §6; what follows is only what you need to read
the panels without being misled.

Rows answer, top to bottom: *is it quoting near fair value and is it
alive*, *what did each tick decide and under which regime*, *how is
inventory tracking*, *are the feeds healthy*. A `Market` selector drives
the per-market panels; the heartbeat, feed-health, and tick-error panels
are process-wide by design.

Four things that read wrong on first contact:

- **A NULL is a fact, not a zero.** A tick can end at six points and
  each knows less than the next, so a column is only populated on the
  paths that could honestly fill it. An unknown skew and a zero skew are
  different, as are an unread vault and an empty one — so nulls are never
  spanned and gaps are left as gaps.

- **A `best bid`/`best ask` series that stops has gone dark**, not to
  zero: a freeze-side reshape, a halt, or a book killed for staleness.
  The touch is derived from the reference resting **on-chain**, not from
  the candidate the tick computed, because that is what a taker can
  actually hit — the gap between the two is the drift the trigger policy
  is tolerating.

- **Feed staleness is the age the engine aged by**, not
  `now() - sample time`. The FX anchor ages from the publisher's clock,
  so a reading received this tick can legitimately be minutes old, and
  over the FX weekend it ages without bound while the crypto-only regime
  carries the mid. That is the signal, not a fault.

- **A leg is a candidate set, not one venue**, so there is still no
  "which feed answered" column. Legs resolve by consensus, and the age
  shown is the *resolved* reading's. Which sources backed a leg — and,
  when they disagreed, which one is the suspect — is the **Leg
  consensus** table. Read `SingleUnverified` there as the *steady state*
  for a market with no second source rather than as a fault: it is the
  only signal that a market is quoted off one unchecked feed, and it
  must never be conflated with `SingleTrusted`.

  Per-source *weights* do now exist, in `maker_leg_contributions`, and
  are plotted on the market-data dashboard's **Fusion weight by source**
  panel. That is not the same claim as "which feed answered" and does not
  reinstate it: the weights describe how the **estimate** was built,
  while `consensus_state` and `contributor_count` describe the **fast
  signal**. The two legitimately disagree — a daily reference fix is
  fused but corroborates nothing, and a trimmed outlier corroborates the
  count but is fused at zero.

- **A dead heartbeat is ambiguous, inherently.** Telemetry is
  fire-and-forget, so it fires identically when the maker has died and
  when the maker is fine but cannot reach Postgres. Feed health
  discriminates in practice: a live bot with a dead database shows every
  feed stale at the same instant.

The feed-health panel is written source-generic, keyed on each source's
own name, so a venue adapter added later appears with no dashboard edit.
Those names are `pub const FEED_NAME` in the `feeds::venues` modules
because the panel joins on them — a renamed source would otherwise empty
a panel with no build error anywhere.

## Alert rules

`provisioning/alerting/maker.yml` carries six rules — dead heartbeat,
stale feed, push-transport down, degraded-or-halted, ticks-failing, and
paused-and-not-quoting — committed for the same reason the dashboards
are: the localnet stack and a hosted stack then alert on identical
conditions.

The rule count is checkable rather than remembered: `sql/alerting/` holds
one mirrored query per rule, named for its uid, and the mirror gate
refuses to go stale.

The ticks-failing rule exists because the others leave a gap that reads
as health: a market whose vault read times out every tick still writes a row
every tick, so the heartbeat is alive; its feeds are fine, so the stale
rule is quiet; and it never reaches the kill-switch policy, so `degraded`
is false and `action` is never `Halt`. It quotes nothing and pages
nobody. The rule keys on `tick_error IS NOT NULL` — not on
`action = 'TickError'`, since a tick that decided and then failed keeps
its decision in `action` — and needs more than half a five-minute window
failing, so one ordinary RPC timeout does not page.

**What provisioning buys and what it does not.** The rules evaluate and
reach Firing in the Alerting UI, which is what makes them checkable from
the stack. They deliver **nowhere**: no contact point is configured, so
they route to Grafana OSS's default and stop. A real destination needs a
secret, and secrets are not committed — wiring one is a deploy-time
concern.

Two authoring notes, both the kind that fail silently:

- **Every rule computes its own window** with `now()` arithmetic rather
  than `$__unixEpochFilter`. The macro's argument capture stops at the
  first closing paren, which is fragile around the
  `extract(epoch FROM now())` these conditions are built from — and an
  alert that fails to interpolate is a monitor that is not watching.
- **Each rule is query → `reduce` → `threshold`.** The reduce is not
  redundant even for a single-row query: a threshold applied straight to
  a table frame is not evaluated per series, and per-series is exactly
  what the three multi-dimensional rules (per feed, per market) need.

**Whether any of this is watching depends on how the maker was started,
and the two paths differ.** The compose `maker-bot` service sets
`DROPSET_DATABASE_URL` with a default pointing at the stack's own
Postgres, so telemetry is **on** for a compose-run maker; unset that
variable to turn it off. A maker run on the host — a bare
`cargo run` of the bot — sets nothing, so telemetry is **off** there
unless you export it yourself.

That difference decides how to read an empty dashboard, and it inverts
between the two. Under compose, an empty heartbeat panel is a **real
fault** — the bot is gone, or its writes are failing. On the host path
it is the expected default and proves nothing. Either way, check the
heartbeat panel first: these rules treat `NoData` as healthy on purpose
(a dashboard outage must never become a trading outage), so "the alerts
are quiet" is only evidence once you know rows are arriving.

Note the alerting mount is why `provisioning/` subdirectories are
mounted by name — see **The tree** above.

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
