<!-- cspell:word bdays -->

<!-- cspell:word Fedwire -->

<!-- cspell:word jiff -->

<!-- cspell:word Juneteenth -->

<!-- cspell:word nager -->

<!-- cspell:word openholidaysapi -->

<!-- cspell:word tzdb -->

<!-- cspell:word usec -->

# The Dropset Market Calendar

The calendar exists to answer two questions, and only two:

1. **Do we expect a leading FX price feed right now?** Interbank FX is
   shut from Friday evening to Sunday evening. In that window the FX
   anchor is *supposed* to be silent, so its silence is scheduled
   information rather than an outage — and the crypto reference, which
   never closes, becomes the price discovery. This is the multi-modal
   quoting regime: exchange-led on weekdays, crypto-led on weekends.
1. **Which sessions are active, and are any of them overlapping?**
   Session overlap is the highest-volatility window of the FX day, and
   the maker should quote wider inside it.

Both are questions about *when an exchange is open*. That framing is
what sets this document's scope, and it is narrower than the scope the
work was originally filed under — see §1.

Everything here is verified against the repo's own stored history
rather than asserted from convention; §6 is the evidence, and the
measurements are reproducible from the queries named there.

## 1. Scope: one dataset, not three

The calendar was originally scoped as three datasets — macro release
events, banking holidays, and the FX session schedule — of which the
first two were to be ingested from providers and the third generated.
Research collapses that to **one generated dataset**: the session
schedule. Neither of the ingested datasets answers either question
above.

That collapse is the single most consequential finding here, because
it removes the entire ingestion surface: no calendar HTTP adapters, no
provider cross-verification, no snapshot-upsert sink, no API keys, and
no third-party availability risk on the quote path. What remains is a
committed rule table, a generator, and one table of absolute UTC
instants.

### 1.1 Holidays are not an availability signal

A currency settlement holiday does **not** close the FX market. FX is
over-the-counter and trades continuously through the week; a holiday
in one currency's home jurisdiction thins liquidity and suspends
*settlement*, but the market stays open and the feeds keep publishing.

This is measured, not assumed. The stored window contains **two** US
federal (and Fedwire) holidays, both falling on a Friday: 2026-06-19
(Juneteenth) and 2026-07-03 (Independence Day observed, since 4 July
fell on a Saturday). A full trading Friday in this series is 1,020
minutes, midnight to the 17:00 ET close, and the nine Fridays run
1,012 to 1,020 bars with a mean of 1,018.4:

| Friday                     | Bars        |
| -------------------------- | ----------- |
| 2026-06-19 (Juneteenth)    | 1,012       |
| 2026-07-03 (4 July obs.)   | 1,018       |
| the seven ordinary Fridays | 1,018–1,020 |

Independence Day observed is indistinguishable from an ordinary Friday.
Juneteenth is the lowest count in the series — and still only 8 bars,
0.8%, below a full day, which is nowhere near a closure and is itself
confounded: 2026-06-19 is the second day of the series, so collector
ramp-up is an equally good explanation. Either reading gives the same
answer. **A US federal holiday does not measurably interrupt the FX
feed.**

Stating it this way rather than as one holiday against a clean
baseline matters, because the earlier framing had the second holiday
sitting *inside* the comparison set.

So holidays are excluded from the calendar's availability path
entirely. If they return later it will be as a *width* input — thin
liquidity is a reason to quote wider, not a reason to switch regime —
and that is the quoting-posture issue's concern, not this one's.

One honest caveat bounds that conclusion: the stored window is June
to August, so it cannot speak to the **Christmas and New Year**
period, which is the one stretch where FX genuinely thins toward
nothing and some venues do stop publishing. That is on the order of
two to four dates a year, trivially authored if it turns out to
matter, and it becomes measurable as soon as the store holds a
December. Re-run the §6.2 daily-profile query then; treat a materially
short 24 or 31 December as the signal that a small authored holiday
set is warranted after all.

### 1.2 Macro releases are out of scope

A scheduled macro release does not change whether a leading feed
exists. It is a volatility and confidence input, which belongs to the
quoting-posture issue, and it is the least urgent of the three
consumers.

There is also a measurement reason to defer it rather than merely
rank it lower: the **hour-of-day volatility profile already absorbs
the habitual release schedule**. US data prints at 08:30 ET, and the
08:00 ET hour is the highest-volatility hour in the stored series by a
wide margin (§6.3). A session-and-hour model therefore prices in the
routine print without knowing any calendar at all. What a macro
calendar would add on top is the ability to distinguish an *unusual*
event day — an FOMC decision, a CPI print — from an ordinary
08:30 ET, which is real but second-order.

The probe evidence is recorded in §1.3 so the work is not repeated.

### 1.3 What was probed, and what it returned

Recorded because a rejected source is worth as much as an accepted
one, and because two of these were previously carried as unverified
assumptions.

| Source                    | Probe result                         |
| ------------------------- | ------------------------------------ |
| OANDA v20 minute bars     | Live; the anchor series, 60,068 bars |
| `date.nager.at`           | Live, keyless; 249 countries         |
| `openholidaysapi.org`     | Live, keyless; **36** countries only |
| ECB TARGET / T2 schedule  | Confirmed: **6** closing days a year |
| ECB monetary-policy dates | **Not probed** — see below           |
| BLS release schedule      | **HTTP 403** — blocked, see below    |
| Federal Reserve FOMC JSON | **No official JSON API exists**      |
| FRED `releases/dates`     | Live, well-formed; needs a free key  |

Five of these need comment. Note first what the two ECB rows are: the
research brief asked for the **ECB calendar** as a macro-release
source, and what was probed is the **TARGET/T2 settlement** schedule —
a different dataset, which belongs to the holiday argument below. The
ECB's monetary-policy meeting calendar was **not** probed, because
macro releases left scope (§1.2) before it was reached. That is a gap
in the record, not a finding: it is named here so the row is not read
as an answer to a question nobody asked.

Two further axes the brief asked for per macro source — **revision
behavior** and **forward horizon** — were likewise not established for
any source, for the same reason. Timezone handling is partially
answered (an authored rule, itself unverified) below. If macro returns,
those three are the first things to establish, not the last.

**`openholidaysapi.org` cannot cross-verify the roster.** It covers 36
countries, and is missing **US, GB, AU, SG, and ID** — five of the
nine currencies' jurisdictions, including the United States, which is
the settlement leg of essentially every pair we quote. The original
design's validation mechanism was "two independent keyless providers,
cross-verified per country-year, agreement is the validation." That
mechanism does not exist for the currencies that matter most. It would
have been discovered at implementation time.

**Market-relevant holidays are not public holidays.** For EUR the
governing calendar is the euro settlement system's, and T2 closes on
exactly six days a year — 1 January, Good Friday, Easter Monday,
1 May, 25 December, 26 December — plus weekends. A public-holidays API
queried for the euro area returns dozens of *national* holidays, none
of which is the relevant fact. Two of the six are Easter-derived and
therefore computed rather than fixed-date. Had the holiday dataset
stayed in scope, its correct sources would have been per-currency
settlement calendars, not per-country public-holiday APIs.

**BLS is not agent-ingestible.** Both the iCalendar endpoint and the
HTML schedule return HTTP 403, including with a browser user-agent,
while egress to other hosts from the same environment succeeds — so
this is bot protection at the host, not a user-agent or network
problem. CPI and the Employment Situation, the two most market-moving
US prints, cannot be polled from the primary source. A collector in
the cloud would hit the same wall.

**The Federal Reserve publishes no FOMC calendar API.** The earlier
working assumption named a "Federal Reserve calendar JSON, probed
reachable, schema unverified" at 538 KB. There is no such official
endpoint for meeting dates; that payload is almost certainly the
Board's general events feed — speeches, conferences, and the like —
which would explain its size and would not have contained the FOMC
schedule. This is what an unopened payload costs, and it is why the
table above records what each source *returned* rather than that it
responded.

Were macro releases to come back into scope, the viable route is
FRED: its API is live, returns well-formed errors, and republishes the
BLS schedules, so it routes around the block for one free key held in
the secrets enclave. Its release dates are expected to be date-only,
so the time of day would come from an authored rule (BLS at 08:30 ET,
FOMC statements at 14:00 ET) — unverified, because confirming it needs
the key.

### 1.4 No Rust crate is fit for this

Surveyed, because adopting beats authoring when something maintained
exists.

| Crate                      | Assessment                                                                           |
| -------------------------- | ------------------------------------------------------------------------------------ |
| `trading-calendar`         | 0.2.3; four releases in two hours on 2025-08-25, nothing since; equity venues; no FX |
| `nyse-holiday-cal`, `usec` | NYSE/US-equity specific                                                              |
| `bdays`                    | Business-day arithmetic; US federal and Brazilian exchange calendars                 |
| `holidays`                 | 0.1.0, single release 2023-02-02                                                     |
| `py-holidays-rs`           | 0.1.3, 2025-09-10; pre-1.0, low adoption                                             |

Two disqualifications recur. Most of these crates model an
**exchange** — NYSE, NASDAQ, LSE — and FX has none: it is OTC, runs
24×5, and its "sessions" are a market convention rather than any
institution's published schedule. And a holiday crate embeds a
**build-time snapshot**, so for a forward-looking calendar its
staleness is bounded by the crate's release cadence rather than the
data's; a three-year-old snapshot is not merely old, it is wrong.

The one crate genuinely worth having is `jiff` (0.2.35, released
2026-07-25, ~55M recent downloads, bundles the IANA tzdb) — but it
answers the *timezone* question, which §2 resolves without a Rust
dependency at all.

## 2. Where daylight saving is resolved: one authority

The FX week boundary is a **local wall-clock** fact, not a UTC one.
Two things support that, and they are worth separating because only
one of them is measured here.

The stored series pins the boundary at exactly Friday 16:59 and Sunday
17:04 New York local time across nine consecutive weekends with zero
variance (§6.1). But every one of those weekends sits inside a single
daylight-saving regime, so the boundary's **UTC** position was equally
constant over the window. Within one regime the local-time and
fixed-UTC models are observationally identical, and this evidence does
not separate them (§6.6).

What selects the local-time model is convention, not the measurement:
the interbank week, the session rule table, and CME's published
schedule are each stated in local time, and the analytics already
resolve them that way. The measurement is *consistent with* that
model, and November is when it becomes a test.

To be precise about what the measurement does and does not settle,
since it is used both ways in this document: it pins the boundary's
**position** hard enough to refute a wrong one — that is how §3.1
rules out CME's Sunday open and §5.2 the hardcoded bracket — but it
cannot discriminate between the local-time and fixed-UTC **encodings**
of that one position, because inside a single DST regime the two
encodings predict identical instants.

The repo was heading for two authorities on this, which would have
drifted:

- `market-data/analytics/session_windows.sql` and
  `weekend_vs_weekday.sql` already resolve sessions and the FX week
  through Postgres `AT TIME ZONE`, against the IANA database, and
  argue explicitly that fixed UTC hours are wrong for most of any
  window longer than a few months.
- The ingestion design specified an authored DST transition table in
  Rust, on a standing "no timezone crate, store UTC instants only"
  decision.

Both are defensible; having both is not. Two encodings of the same
session rules, maintained separately, disagree eventually — and the
disagreement surfaces as a regime mis-classification, which is the
expensive failure (§6.5).

**The resolution: Postgres is the only DST authority.** It already is
one, for the analytics. The generator runs there, resolves local wall
clock through the IANA database, and materializes **absolute UTC
instants** into the calendar table. Everything downstream consumes
instants.

This preserves both standing decisions rather than trading one away:

- `feeds/src/time.rs` stays UTC-only with no timezone dependency. Its
  existing civil-date arithmetic is exact and needs nothing added.
- The maker never resolves a timezone, and never queries Postgres on
  the quote path — it reads pre-computed instants through the
  slow-variable path, exactly as it reads any other slow variable.
- The session rules have one home, shared with the analytics that
  already validate against them.

The cost, stated because it is a real one: the Postgres image's tzdb
version becomes a correctness input. DST rule changes — a jurisdiction
abolishing the practice, say — arrive via image updates rather than a
crate bump or a hand edit. That is a smaller and more visible surface
than a hand-maintained transition table, but it is not zero, and it
belongs in the image-bump checklist.

## 3. The session model

### 3.1 The FX week

The interbank week runs **Sunday 17:00 to Friday 17:00 New York
time**. That is the convention, and it is the boundary the
availability question turns on. The stored series matches the Friday
close exactly and the Sunday open to within four minutes — the anchor's
first bar lands at 17:04, which §5.2 argues is structural and ratifies
as the instant the generator actually emits. So "17:00" below means the
convention; where the four minutes matter, they are called out.

Under the local-time model its UTC position moves with US daylight
saving: Friday close at 21:00 UTC and Sunday open at 21:00 UTC while
New York is on EDT, both an hour later on EST. That movement is a
property of the model rather than something this window observed —
every stored weekend is EDT (§2, §6.6) — and it is why the boundary is
generated from local wall clock rather than written down in UTC.

**Why this is derived from the tape and not cited from a venue.** No
institution publishes the interbank week — there is no exchange to
publish it. The closest authoritative schedule that does exist is
CME's for FX futures: Euro FX trades Sunday 17:00 to Friday 16:00
Central, with a daily 16:00–17:00 Central halt. Checked against the
measured boundary, it is half right:

|              | CME published       | Measured interbank | Agreement |
| ------------ | ------------------- | ------------------ | --------- |
| Friday close | 16:00 CT = 17:00 ET | 17:00 ET           | exact     |
| Sunday open  | 17:00 CT = 18:00 ET | 17:04 ET           | ~1 h late |

So adopting the one published schedule available would reproduce
almost exactly the defect §5.2 documents — an hour of live market
classified as closed, on a Sunday. CME also halts for an hour every
weekday, which the interbank tape does not: the stored weekdays carry
~1,430 of 1,440 minutes with no hour-long gap anywhere in them.

That is the case for empirical derivation in one line: the published
schedule describes a *different market* that happens to trade the same
currencies.

Worth noting because the maker's own comment currently conflates the
two, describing interbank FX and CME as shut on one shared
Friday-to-Sunday-17:00-ET bracket. Read as a statement about
**interbank**, that bracket is right — it is CME that does not fit it,
reopening an hour later. So the correction is to stop describing the
two as one schedule; it is emphatically **not** to move the Sunday
open to 18:00 ET, which would introduce precisely the hour-late error
§5.2 measures.

### 3.2 The four sessions, and their overlaps

The rule table already exists, in `session_windows.sql`, as local
opening hours in the city that defines each session — Sydney 08:00 to
17:00, Tokyo 09:00 to 18:00, London 08:00 to 17:00, New York 08:00 to
17:00, each in its own zone, close hour exclusive. The generator reads
the same table; it is not restated in Rust.

Overlaps are derived, not authored: two sessions overlap when their
generated instants intersect. The one that matters most is
**London/New York**, which the rule table places at 08:00 to 12:00 ET
in summer — and which the stored series independently confirms as the
highest-volatility band of the day, though at ~1.7× the median hour
rather than the larger figure the raw peak suggests (§5.3 derives the
multiplier and §6.3 the caveat behind it). A bar may be counted under
more than one session, and overlaps inherit that: the analytics
already accept multiple counting across sessions, deliberately, and
note that per-session counts therefore sum to more than the total.

### 3.3 What "closed" means, precisely

Only the **weekly** boundary closes the market. Nothing else in this
calendar does:

- The weekend closes it. This flips the regime.
- A holiday does not (§1.1). At most it widens.
- A macro release does not. At most it widens.

Keeping that distinction sharp is what stops the calendar from
mis-firing the multi-modal switch on a day the market is merely quiet.

## 4. Ingestion: there is none

With holidays and macro releases out of scope, **no dataset is
ingested**. This is a substantial simplification of the original
design and it should be taken deliberately rather than by omission:

- No calendar adapters under `feeds/`. The framework's Source/Sink
  seam is not involved, because nothing is polled.
- No snapshot-upsert sink. It was needed to replace a provider's
  forward window each poll; with no provider there is no window to
  replace.
- No API key, no secrets-enclave entry, no per-venue budget, no
  provider outage to fall back from.

What is built instead:

1. **A generator**, in `market-data/`, that expands the committed rule
   table over a rolling forward horizon and writes absolute UTC
   instants. It runs on a slow cadence — the horizon is known years
   ahead, so daily is generous — and is idempotent: regenerating a
   window that already exists must be a no-op, so the run can be
   repeated freely.
1. **One table**, additive-only, via the schema owner, holding typed
   instants: session opens and closes, and the weekly close and open.

Because the horizon is generated rather than fetched, the failure mode
is not an outage but an **expiry**: if the generator stops, the
horizon runs out silently. The table must therefore carry how far it
has been generated, and a consumer reading past that point must treat
the calendar as unavailable and fall back (§5.3) rather than infer
"closed" from an absent row. Absence of a future instant is ignorance,
not closure.

### 4.1 Naming: do not call it `events`

The shared database **already has an `events` table** — the indexer's
on-chain event log, keyed by slot, transaction index, signature, and
event ordinal. The original design proposed "an events table" for
macro releases, which would have collided outright in the one shared
`dropset` database. The data-feeds doc avoided that collision, naming
its planned table `fx_events` — though note that row is now **moot in
substance**, since §1.2 removes the dataset it was planned for; it is
cited here only as the better naming choice, and §4.2 reconciles it.

The macro table is out of scope here, so the collision is moot for
now, but the name is reserved and the calendar's own table must not
reach for it either. Any calendar table should be prefixed
accordingly.

A smaller adjacent finding, noted only because this is where someone
would look for it: the running database also holds a `feed_health`
table that **nothing in the repo references** — not the source, not
the migrations, both searched. How it got there is not established.
It is unrelated to the calendar but is exactly where a feed-health
consumer would reach by mistake, so it wants a deliberate
drop-or-reclaim. It is tracked as its own issue rather than left
resting in this spec.

### 4.2 Reconciling the sibling documents

Deferring the macro dataset invalidates several live statements in
`docs/data-feeds.md`, and those are reconciled **in this change**
rather than handed onward. That choice is deliberate: the natural
place to delegate the cleanup would be the ingestion issue, but §1 and
§4 have just emptied that issue's subject, so it may well close as
"nothing left to build" and take the delegated cleanup with it. A doc
fix that depends on an issue this document dissolves is a doc fix that
does not happen.

What changed there:

- The consumers list no longer promises an econ-calendar feed, and
  points here instead.
- `fx_events` is marked *(deferred)* rather than *(planned)*, and a
  `fx_sessions` row is added for the one table that is actually
  coming.
- The venue-policy table marks the macro overlay deferred, and gains a
  short subsection stating that the **session clock is not a leg
  source** — deliberately *not* a row in that table. Every row there is
  a price source offered to a leg's consensus, and a clock carries no
  value to corroborate, so it belongs to no leg. An earlier draft of
  this change did add it as a row; that was wrong, and the correction
  is the shape the quoting-posture issue consumes.
- The polling-cadence table's econ-calendar row is removed rather than
  rewritten, with the generated instants' exemption stated in prose
  beneath it — a non-polled artifact given a cadence and a collector
  owner would have reproduced, relabelled, the same category error the
  old row made against §4's "nothing is polled".
- The §13 open question "Econ-calendar source" is **resolved**, in the
  same form as the resolved FX-bar-source entry beside it, and keeps
  the two probe findings (BLS blocked, no Fed API) that cost a probe
  each.

`docs/market-making.md` was checked and needs **nothing**: it carries
no econ-calendar pointer, and its two macro references describe
volatility behavior and the FX oracle's own confidence widening, which
hold with or without a calendar. Its regime section still wants the
multi-modal posture written up, and that stays with the
quoting-posture issue, which is the change that implements it.

## 5. Consumers

### 5.1 Expected unavailability already works

This is worth stating plainly because the work was filed on the
premise that nothing implements it: **the distinction between "dark
because closed" and "dark because broken" is already built**, in
`fair-value/src/engine.rs`. With no live FX leg, `compose` returns
`Regime::CryptoOnly` with `Health::Ok` when the weekend flag is set —
structural, healthy, the crypto reference becomes the anchor — and
`Regime::Degraded(Degrade::FxStale)` with `Health::Degraded` when it is
not. The engine's own documentation calls a permanent condition
reported as a fault "a fault the operator learns to ignore."

The supporting machinery is in place too, and is more thorough than
the filing suggested:

- Pyth readings age from `publish_time` rather than receipt,
  specifically so a frozen Friday close does not read as perpetually
  fresh and prevent the weekend flip.
- The receipt-aged FX fallback is *suppressed* while the session is
  shut, so a source that keeps serving stale weekend rates cannot hold
  the engine in the Normal regime on a dead market.
- The OANDA collector already treats an empty weekend window as
  legitimate and advances its cursor through it, so an idle Saturday
  is not a stall.

So what the calendar changes here is **not the mechanism but its
clock**. Today the weekend flag comes from a hardcoded UTC
approximation; the calendar makes it authoritative. That is a much
smaller and better-defined change than "specify expected
unavailability from scratch," and it means the consumer work is a
substitution with a cross-check, not a new subsystem.

One gap does remain, and it is genuinely unimplemented: the
*reporting* surface. Operator-facing health — the TUI, alerts — has no
notion of scheduled silence, so a closed-market OANDA still looks
dark there even though the engine correctly reads it as healthy. Per
the standing surface split, health and telemetry are Grafana's, and
this is where the "unsurprised when OANDA goes dark" requirement
actually needs building.

### 5.2 The measured defect in today's approximation

The current weekend flag is a hardcoded UTC bracket — Friday 21:00 to
Sunday 22:00 UTC, DST explicitly ignored, thresholds marked as to be
determined. Against the measured boundary it is wrong by about an hour
a week, and the error is seasonal:

|              | Approximation | Convention | First bar seen |
| ------------ | ------------- | ---------- | -------------- |
| Friday close | 21:00 UTC     | 21:00 UTC  | 20:59 (last)   |
| Sunday open  | 22:00 UTC     | 21:00 UTC  | **21:04**      |

Against the convention boundary the Sunday error is a round **60
minutes**; against the first bar actually observed it is ~56. Both
numbers are defensible because there are two references, and which one
governs is settled at the end of this section rather than left
implicit. In winter
the boundary shifts an hour later in UTC and the error swaps ends: the
Friday close becomes ~60 minutes early and the Sunday open lands on
the convention boundary exactly. So one boundary is right and the
other is about an hour wrong, in each half of the year.

The direction matters more than the magnitude. Measured against the
**convention** boundary, the approximation's window is a superset of
the true window in both seasons and on both transition weekends, so
its errors declare the market **closed while it is open**. That is the
safe direction: it cannot spuriously degrade the engine or tighten the
kill switches on a live market. What it does instead is **mask a real
outage** for that hour — a genuine FX failure inside the window reads
as a healthy scheduled closure. That is why the defect has gone
unnoticed, and also why it is worth fixing rather than tolerating.

**One exception, and it runs the unsafe way.** Measured instead
against the boundary this document goes on to ratify — feed
availability at 17:04, below — the superset property fails at one end
for half the year. On an EST Sunday the approximation reopens at 22:00
UTC, which is 17:00 EST, while the anchor does not publish until 17:04
EST. For those four minutes it declares the market **open while the FX
leg is provably absent**, which composes to `Degraded(FxStale)` and
tightens the kill switches (§6.5). That is roughly 22 Sundays a year.
So the approximation is not uniformly conservative after all: it is
conservative by about an hour on one edge and wrong by four minutes on
the other, and which edge depends on the season.

This has a concrete consequence, because §5.3 keeps the approximation
as the **permanent** fallback. Its Sunday reopen should move to
**22:05 UTC** as part of the substitution, which restores the superset
property under the ratified reference and costs only five more minutes
of scheduled-silence masking. Without that change the fallback carries
a weekly false-degrade window, and the promotion cross-check would
correctly flag it as *unexpected* disagreement — the known seasonal
hour it is told to ignore sits on the Friday edge, not this one.

**Which instant the generator should emit — 17:00 or 17:04.** The four
minutes are not noise, and the choice has a consequence. Across all
nine Sundays the first bar lands at 17:04 ET with **zero** bars at
17:00 through 17:03; minute 04 is present on 9 of 9, while later
minutes in the same hour are missing on some (17:06 on 7 of 9). A
liquidity gap would vary — this does not, so 17:04 is a structural
property of the anchor's reopen rather than a quiet opening few
minutes.

That settles the choice. The consumer is *do we expect a leading
feed*, not *is the market notionally open*, and those differ by four
minutes every Sunday. Emitting the convention 17:00 would have the
maker expect an anchor that provably is not there yet — and a missing
FX leg outside the weekend window composes to
`Degraded(FxStale)`, with the kill-switch tightening of §6.5 behind
it. That is a false degrade, weekly, introduced by the fix. So the
generated open is the **feed-availability** instant, and the
convention boundary is recorded beside it as documentation rather than
used.

The cost, stated because it is real: this makes the weekly open a fact
about *our anchor* rather than about the market, so it must be
re-derived if the anchor vendor changes. That is the one place this
calendar deliberately describes the feed instead of the market, and it
is why §6.1's acceptance test is written against observed gaps.

This document specifies the fix but does not implement it; the
substitution belongs to the quoting-posture issue, where the
cross-check below governs the promotion.

### 5.3 Quoting posture

The calendar feeds the clock context the quoting-posture issue
defines, and the mapping follows §3.3 exactly:

- **Market closed** — the multi-modal switch. FX is expected silent,
  the crypto reference leads, and the engine's existing crypto-only
  regime is the correct healthy state. It is **also a widen**, which
  is the operator's first motivating example and is easy to lose
  behind the regime switch: see the policy below.
- **Session overlap** — widen, per the table below.
- **Calendar unavailable or expired** — fall back to the existing
  approximation, loudly, and never to "open" and never to a halt.

**The widening policy per state.** All three act through one lever —
inflating the confidence half-width the existing uncertainty
machinery already consumes — so the policy is a multiplier on that
half-width per clock state. Derived from §6.3, whose median hourly
sigma over the stored series is 0.700 bps/min:

| Clock state                       | Measured sigma                  | × median | Policy                                            |
| --------------------------------- | ------------------------------- | -------- | ------------------------------------------------- |
| London/NY overlap, 09:00–11:00 ET | 1.180 mean                      | 1.69×    | widen ~1.7×                                       |
| The 08:00 ET release hour         | 1.755                           | 2.51×    | widen ~2.5× — **release absorption, not overlap** |
| Post-close lull, 17:00–19:00 ET   | 0.464 mean                      | 0.66×    | **no change** — floor at 1.0                      |
| Sydney/Tokyo overlap              | not measured                    | —        | unpriced — see below                              |
| Market closed (weekend)           | not derivable from any FX sigma | —        | widen; a risk budget, not a measurement           |

Five things this table is careful about, because the naive reading of
§6.3 overstates the case:

- **The 4.5× figure is not the overlap effect.** It is the ratio of
  the single 08:00 hour to the single 19:00 hour, and 08:00 is
  confounded — it carries the 08:30 ET US release as well as the
  overlap open (§1.2 leans on exactly that). The overlap-only
  elevation, taken from the hours that are *not* confounded, is
  ~1.7×. Hour 11 (0.978) is statistically indistinguishable from
  non-overlap hour 04 (0.982), so the band is not uniformly elevated
  either. The widening rule survives — 08:00–10:00 are the series'
  top three hours — but at ~1.7×, not 4.5×.
- **Never tighten on a clock signal alone.** The lull is measurably
  quieter, and the policy still floors the multiplier at 1.0: a quiet
  clock is not evidence that *this* market is quiet, and the downside
  of quoting too tight is unbounded where quoting too wide costs
  volume.
- **The closed-market multiplier is not a volatility measurement at
  all**, and this is the row most likely to be got wrong. It cannot
  come from the FX anchor, which is silent by definition. But it must
  not come from realized volatility on the *crypto* leg either: a
  weekend tape is quiet, so any sigma-driven calibration returns a
  multiplier **below** 1.0 — a tightening — which is exactly the trap
  §5.4 names, and the honest weekend tape in §6.4 (6.9 bps across a
  whole Saturday, against weekday hours averaging 0.700 bps per
  *minute*) would produce precisely that. The quantity that actually
  rises when interbank shuts is not realized volatility but
  **exposure we cannot hedge**: with no arbitrage channel the basis is
  free to drift, and nothing closes it until Sunday. So this constant
  is a **risk budget** — how much adverse drift we are willing to be
  wrong by with no way to hedge — informed by the weekend basis
  deviation series' observed *range*, not by its sigma. Naming it a
  measurement is what would make it wrong; it is deliberately left as
  a decision.
- **Three of the four sessions are unpriced.** §3.2 generates all four
  sessions and derives every overlap, but only London/NY has been
  measured (§6.3 is a single ET-hour profile). The Sydney/Tokyo
  overlap is a real generated clock state with no evidence and no
  multiplier behind it. Since "which sessions are overlapping" is half
  this calendar's purpose, that gap is named in §6.6 rather than
  papered over with a borrowed constant.
- **These are calibration inputs, not final constants.** The
  quoting-posture issue owns the constants and their tests; this spec
  owns the shape, the derivation, and the floor.

The promotion path is a cross-check, not a swap: run the
calendar-derived state alongside the existing approximation, alarm on
disagreement, and rewire only once the generated instants have been
validated against the stored series. The two are already known to
disagree for about an hour a week (§5.2), so the alarm must be keyed
to *unexpected* disagreement — the known seasonal hour is the
approximation being wrong, not the calendar.

### 5.4 The estimator and analytics

The calendar contributes the regime label the analytics already
compute for themselves. Once it exists, `weekend_vs_weekday.sql` and
`session_windows.sql` should read the generated instants instead of
recomputing the boundary inline, so the definition has one home. That
is a refactor of existing queries, not new analysis, and it is how the
generator earns its correctness guarantee: the analytics become
consumers of the thing they were used to validate.

For the estimator, the calendar supplies session-aware volatility
bucketing (§6.3 is the shape of it) and the weekend regime flip. It
should **not** supply a weekend volatility figure derived from a
closed-market anchor: a frozen or indicative series yields a
plausible-looking ultra-low sigma for a session that never traded.
Prefer a source whose weekend bar count is zero — absence is the
honest signal (§6.4).

**What the calendar contributes to the fused fair price's
confidence**, stated separately because it is the part most easily
assumed: nothing to the point estimate, and three things to the
half-width.

1. **Which legs are *expected* present.** This is the load-bearing
   one. Confidence should reflect that a leg is absent-by-schedule
   rather than absent-by-fault — the same distinction §5.1's regimes
   draw — so a closed market yields a wider band around a
   crypto-anchored mid, not a narrower one around a stale FX print.
1. **A sigma to scale it by**, per the §5.3 table.
1. **Nothing when the calendar itself is unavailable.** Past its
   generated horizon the calendar contributes no confidence term at
   all and the fallback governs; it must never contribute a
   *default* term, which would read as knowledge it does not have.

The trap worth naming: a weekend tape is quiet, so a sigma-driven
half-width computed naively over weekend data gets **tighter** exactly
when the anchor has gone. That is backwards — with interbank shut
there is no arbitrage channel, so the basis is free to widen — and it
is the concrete reason the closed-market state is a widen rather than
a recomputation.

## 6. Verification

The calendar is generated, so it cannot be validated against a
provider. It is validated against **our own stored history**, which is
a stronger test: the generated instants must reproduce the structure
the tape already shows. All figures below come from the OANDA EUR/USD
minute series, 60,068 bars spanning 2026-06-18 to 2026-08-17, unless
noted.

### 6.1 The weekly boundary

Consecutive-bar gaps longer than six hours, over the whole series.
Nine gaps, one per weekend, and every one identical:

| Last bar (ET) | Next bar (ET) | Gap     |
| ------------- | ------------- | ------- |
| Friday 16:59  | Sunday 17:04  | 48.08 h |

Nine of nine, zero variance, in New York local time. In UTC the same
nine collapse to Friday 20:59 and Sunday 21:04 — consistent only
because the whole window is EDT.

The acceptance test for the generator is this query: its emitted
weekly close and open instants must bracket every observed gap, with
no gap left outside a generated closed window and no generated closed
window containing bars.

### 6.2 The daily bar profile

Bar counts per local day reconstruct the entire week boundary
independently, without reference to any external schedule:

| Day             | Days | Bars        | Of possible | Coverage |
| --------------- | ---- | ----------- | ----------- | -------- |
| Monday–Thursday | 32   | 1,421–1,436 | 1,440       | ~99.3%   |
| Friday          | 9    | 1,012–1,020 | 1,020       | ~99.8%   |
| Sunday          | 9    | 405–414     | 416         | ~98.5%   |

Friday's 1,020 is midnight to the 17:00 ET close. Sunday's denominator
is **416**, not 420: the anchor's first bar is 17:04 (§5.2), so
17:04–midnight is 416 minutes. Using 420 understates Sunday coverage
as ~97.5%.

**Two partial days are excluded from these ranges**, and saying so
matters because this table is a re-runnable acceptance artifact: the
series' first day (2026-06-18, 286 bars) and last (2026-08-17, 1,151)
are both cut off by the collection window, so the Monday–Thursday row
covers 32 full days out of the 34 in the span. The 52 days shown sum to
the stated 60,068. Note the short first day also lends weight to the
collector ramp-up explanation §1.1 offers for the 2026-06-19 Friday.

The profile reconstructs the §6.1 boundary from a completely different
statistic. This is also the query to re-run once the store holds a
December, per §1.1.

### 6.3 The overlap volatility band

Realized volatility from adjacent-bucket log returns, bucketed by New
York local hour, in basis points per minute:

| ET hour | Vol       | Note                                        |
| ------- | --------- | ------------------------------------------- |
| 08:00   | **1.755** | London/NY overlap opens; US prints at 08:30 |
| 09:00   | 1.286     | overlap                                     |
| 10:00   | 1.276     | overlap                                     |
| 11:00   | 0.978     | overlap, decaying                           |
| 12:00   | 0.772     | London closes                               |
| 04:00   | 0.982     | London opens                                |
| 19:00   | **0.392** | post-NY-close trough                        |

The elevated band is 08:00 to 11:00 ET and it drops at 12:00. Its
**edges** line up exactly with the London/New York overlap the rule
table predicts (London 08:00–17:00 BST is 03:00–12:00 ET; New York is
08:00–17:00 ET; the intersection is 08:00–12:00 ET, and under
close-hour-exclusive semantics that is hours 08 through 11).

Three things follow, and the third limits the first two.

1. The overlap-widening rule is confirmed on our own data rather than
   adopted on convention — the predicted window and the measured band
   coincide at the edges.
1. The 08:00 spike carrying the 08:30 print is the concrete basis for
   §1.2: the hour-of-day profile already absorbs the routine release.
1. **Those two claims lean on the same hour, so neither may use the
   raw peak.** Hour 08 is both the overlap open and the release hour,
   and it alone produces the 4.48× peak-to-trough ratio. A single
   datum cannot be evidence that the profile absorbs the release *and*
   the measure of the overlap effect. The band is also not uniformly
   elevated: hour 11 (0.978) is indistinguishable from non-overlap
   hour 04 (0.982). So the overlap multiplier is taken from the hours
   that are not confounded, at ~1.7× the median (§5.3), and the 4.48×
   figure is reported here as the profile's full dynamic range and
   used for nothing else.

**Provisional, and deliberately labelled so.** These figures come from
a single ~60-day window on one pair. `docs/data-feeds.md` §13 carries
an open question requiring a stated history depth per estimate, below
which a number is reported as provisional rather than used to set a
band; that depth does not exist yet, so this band is provisional by
that rule. It is strong enough to justify the *shape* of the widening
policy and to order the hours; it is not yet enough to fix a constant,
which is why §5.3 hands the constants to the consumer issue.

### 6.4 Absence versus silence

Over one weekend window, Friday 21:00 to Sunday 21:00 UTC:

| Series                    | Bars      | Distinct closes | Range     |
| ------------------------- | --------- | --------------- | --------- |
| OANDA EUR/USD and AUD/USD | **0**     | —               | —         |
| Twelve Data AUD/USD       | **2,880** | 110             | 23.4 bps  |
| Coinbase AUDD/USDC        | 19        | 12              | 130.8 bps |

2,880 is exactly 48 × 60: an **unbroken** minute grid across a market
that was shut. OANDA's zero over the same window is the other extreme.

**The first conclusion is the safe one.** Weekend coverage is a vendor
convention, not a fact about the market, so the anchor's silence is
the trustworthy signal: session detection reads bar *absence* from a
source that honestly reports it, never trusts a complete grid, and
never pools the two into one statistic. That is what §5.1 and the
generator rely on, and nothing below weakens it.

**The second conclusion is that telling a synthetic grid from an
honest one is harder than this table makes it look**, and the
comparison above is the wrong one to draw it from. AUDD/USDC is the
roster's thinnest tape — 19 bars is 0.66% coverage — so setting 2,880
against it implies a discriminator far cleaner than the data supports.
Measured against a genuinely liquid 24/7 venue over one Saturday
(00:00–24:00 UTC), and against the suspect series' own Saturday
interior, the picture narrows sharply:

| Saturday only       | Bars of 1,440 | Distinct closes | Range    |
| ------------------- | ------------- | --------------- | -------- |
| Twelve Data AUD/USD | **1,440**     | 53              | 11.6 bps |
| Coinbase EURC/USDC  | 1,122 (77.9%) | 9               | 6.9 bps  |

Two of the three columns fail as tests, and one fails *backwards*:

- **Magnitude does not separate them, and points the wrong way.** The
  suspect series moved *further* than the honest one — 11.6 against
  6.9 bps. Two confounds to state, because they bound how much this
  shows: the two Saturdays are a week apart (2026-08-15 and
  2026-08-08), and the instruments differ (AUD/USD against
  EURC/USDC), whose natural weekend movement need not match. What
  survives the confounds is the weaker but sufficient claim —
  magnitude is not a clean discriminator, since the suspect series is
  not even on the quiet side of an honest one. This matters beyond
  this document,
  because magnitude is the test `weekend_vs_weekday.sql` adopted after
  rejecting `distinct_closes` — it records the same vendor and pair
  moving "about 0.7 bps across its entire Saturday", and that figure
  **does not reproduce** on the stored series. The disagreement is not
  a windowing artifact: the interior figure above deliberately excludes
  both session boundaries, so it is not inflated by the Friday-evening
  or Sunday-evening edges. Which measurement is right is unresolved and
  is flagged in §6.6; until it is, no conclusion should rest on the
  0.7 bps figure or on magnitude generally.
- **Distinct closes does not separate them either** — the honest tape
  shows *fewer* (9 against 53). That is the outcome
  `weekend_vs_weekday.sql` already predicted when it rejected this
  heuristic as firing on honest data, and this measurement agrees with
  the rejection.
- **Grid completeness is the column that does separate them**:
  100% against 77.9%. A real venue emits no candle for a minute with
  no trades, so an unbroken weekend is the anomaly. Calibrate the
  threshold from *our* measurement, not the sibling's: this Saturday's
  honest gap rate is 22.1%, where `weekend_vs_weekday.sql` puts the
  ordinary rate near 12% across a 60-day window. That is a second
  figure from the same file that does not reproduce here, and it is
  named rather than leaned on — the gap rate plainly varies by venue,
  pair, and window, which is itself why a threshold needs more than
  one weekend behind it.

So the hypothesis this document records — and deliberately does **not**
make load-bearing — is narrower than the first table suggests:
completeness is the only one of the three columns measured here that
separates the series, and its real separation is 100% vs ~78%, not
100% vs 0.66%. "Of the three measured" is the honest scope; a related
statistic in the same data looks promising and is untested — 53
distinct closes across 1,440 bars means ~96% of minutes repeat the
prior close, so a repeat-run-length or distinct-per-bar rate (3.7% vs
0.8%) separates in the same direction. Note that is not the rejected
heuristic: `weekend_vs_weekday.sql` rejected the raw distinct *count*,
not a rate.

The bar for adoption is set accordingly: reproduce it across several
vendors and several weekends, calibrate against ~78% rather than
against a thin tape, and resolve the magnitude disagreement above
first. Until then the roster's per-source weekend behavior is authored
knowledge, not detected — and absence, not completeness, is what the
calendar actually depends on.

### 6.5 What a wrong calendar costs

Stated concretely, because "the calendar must be right" needs a
blast radius to be actionable. The weekend flag is load-bearing in
three places, and a mis-classification propagates to all of them:

1. **The regime.** Closed-when-open masks a real FX outage as healthy
   (§5.2). Open-when-closed is worse: it degrades the engine on a
   normally-shut market, every week. That one is not hypothetical —
   today's approximation does it for four minutes on every EST Sunday
   (§5.2), which is why the fallback bracket wants widening rather
   than keeping as-is.
1. **The FX fallback.** The flag gates suppression of the
   receipt-aged fallback. Wrong in one direction, a stale weekend rate
   anchors the mid; wrong in the other, a usable fallback is
   discarded.
1. **The kill switches.** A degraded composition tightens the whole
   switch set. So a calendar that reports open-when-closed does not
   merely mislabel a state — it moves the imbalance thresholds and the
   TVL drawdown floor, every weekend.

This asymmetry is why the fallback is to the approximation and never
to "open," and why the promotion path is a cross-check rather than a
swap.

### 6.6 What is not yet verified

Seven gaps, named rather than papered over. The first two are the
serious ones, because they are about the generator's own output rather
than about deferred datasets:

- **Only the weekly boundary has an acceptance test.** §6.1 tests the
  weekly close and open against observed gaps. The generator's *other*
  output — four sessions' opens and closes, and the overlaps derived
  from them — has no specified test at all. That is roughly half of
  what the calendar emits, and it is the half feeding the overlap
  widen. A test exists in principle along the same lines (generated
  session instants should bracket the shifts in the ET-hour profile),
  but it is not written here.

- **Three of the four sessions are unmeasured.** Only London/NY
  appears anywhere in §6; Sydney, Tokyo, and their overlap are
  authored from the rule table and never checked against the tape.
  Sydney and Tokyo also sit in the hours this window covers least
  convincingly, since the ET-hour profile pools all days.

- **No DST transition is inside the stored window.** The nine
  weekends are all EDT, so the boundary's stability in *both* ET-local
  and UTC terms is confirmed and the two models are not
  distinguished — see §2, which relies on convention rather than this
  measurement to choose between them. The local-time model predicts
  the UTC boundary moves an hour in November; that becomes testable
  then, and it is the single most valuable outstanding check.

- **A sibling measurement disagrees with §6.4 and is unresolved.**
  `weekend_vs_weekday.sql` records the Twelve Data AUD/USD weekend
  series moving ~0.7 bps across a Saturday; the stored series gives
  11.6 bps over a Saturday interior that excludes both session
  boundaries. A factor of ~16, same vendor and pair. Both cannot be
  right about the same behavior, and the sibling doc's magnitude test
  rests on its figure. Resolving this needs the window and date of the
  original measurement, which are not recorded there. Until then,
  neither figure should be used to classify a series, and the
  completeness hypothesis is held at arm's length for the same reason.

- **The 08:00 ET hour is confounded** between the overlap open and the
  08:30 release, and this window cannot separate them (§6.3). Doing so
  needs either a macro calendar — deferred — or enough history to
  compare release days against release-free ones at the same hour.

- **Christmas and New Year are unobserved** (§1.1).

- **FRED's release-date granularity is unconfirmed**, needing a key
  (§1.3). Immaterial while macro is out of scope.

## 7. Open questions

- **Does the horizon need history at all?** Generated session instants
  are derivable at any time from the rule table, so past rows are a
  convenience for the analytics rather than an irreplaceable record —
  unlike price history, which cannot be reconstructed. If the
  analytics read instants directly, retention matters; if they join
  against them, a rolling window suffices. This is a narrow instance
  of `docs/data-feeds.md` §13's still-open **Retention** question and
  should be decided with it rather than separately — the calendar is
  the easy case, since it can be regenerated.
- **Do holidays come back as a width input?** Deferred, not refused
  (§1.1). The December re-test decides it.
- **Where does scheduled silence surface to an operator?** §5.1 names
  this as the one piece genuinely still missing. It is a Grafana
  question
  under the standing surface split, and it wants its own scope.
