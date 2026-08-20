<!-- cspell:word bdays -->

<!-- cspell:word Fedwire -->

<!-- cspell:word jiff -->

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

This is measured, not assumed. Friday 2026-07-03 was US Independence
Day observed (4 July fell on a Saturday), a Federal Reserve and
Fedwire holiday — the most significant US market holiday inside the
stored window. The OANDA EUR/USD minute series published **1,018 bars**
that day, against a mean of ~1,019 across the nine other Fridays in
the series (1,012 / 1,018 / 1,019 / 1,020). The holiday is invisible
to feed availability.

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
| BLS release schedule      | **HTTP 403** — blocked, see below    |
| Federal Reserve FOMC JSON | **No official JSON API exists**      |
| FRED `releases/dates`     | Live, well-formed; needs a free key  |

Four of these need comment.

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
That is measured: across nine consecutive weekends the boundary sits
at exactly Friday 16:59 and Sunday 17:04 New York local time with
zero variance, while its UTC position moves with US daylight saving
(§6.1).

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
time**. This is the boundary the availability question turns on, and
it is confirmed exactly by the stored series (§6.1).

Expressed in UTC it moves with US daylight saving: Friday close at
21:00 UTC and Sunday open at 21:00 UTC while New York is on EDT,
both an hour later on EST. This is precisely why it is generated from
local wall clock rather than written down in UTC.

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
currencies. Worth noting because the maker's own comment currently
conflates the two, describing interbank FX and CME as shut on the same
Friday-to-Sunday-17:00-ET bracket; the Friday half is right and the
Sunday half is CME's 18:00 ET, not 17:00.

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
highest-volatility band of the day, at up to 4.5× the daily trough
(§6.3). A session may be counted in more than one overlap; that is
deliberate, the same convention the analytics use.

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
`dropset` database. `docs/data-feeds.md` §8 gets this right, naming
the planned table `fx_events`.

The macro table is out of scope here, so the collision is moot for
now, but the name is reserved and the calendar's own table must not
reach for it either. Any calendar table should be prefixed
accordingly.

A smaller adjacent finding, recorded in passing: the running database
also holds a `feed_health` table that **nothing in the repo
references** — not the source, not the migrations. It is an orphan
left in the durable volume by a migration that has since gone. It is
unrelated to the calendar, but it is the natural place a feed-health
consumer would reach for, so it should be dropped or reclaimed
deliberately rather than adopted by accident.

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

|              | Approximation | Measured (EDT) | Error        |
| ------------ | ------------- | -------------- | ------------ |
| Friday close | 21:00 UTC     | 21:00 UTC      | none         |
| Sunday open  | 22:00 UTC     | **21:04 UTC**  | ~56 min late |

In winter the boundary shifts an hour later in UTC and the error
swaps ends: the Friday close becomes ~60 minutes early and the Sunday
open lands exactly. So one boundary is right and the other is about an
hour wrong, in each half of the year.

The direction matters more than the magnitude. Both errors declare the
market **closed while it is open** — never the reverse. That is the
safe direction: it cannot spuriously degrade the engine or tighten the
kill switches on a live market. What it does instead is **mask a real
outage** for that hour: a genuine FX failure inside the window reads
as a healthy scheduled closure. That is why the defect has gone
unnoticed, and also why it is worth fixing rather than tolerating.

This document specifies the fix but does not implement it; the
substitution belongs to the quoting-posture issue, where the
cross-check below governs the promotion.

### 5.3 Quoting posture

The calendar feeds the clock context the quoting-posture issue
defines, and the mapping follows §3.3 exactly:

- **Market closed** — the multi-modal switch. FX is expected silent,
  the crypto reference leads, and the engine's existing crypto-only
  regime is the correct healthy state.
- **Session overlap** — widen. The empirical basis is §6.3, and it is
  strong: a 4.5× spread between the busiest and quietest hour is not a
  marginal effect.
- **Calendar unavailable or expired** — fall back to the existing
  approximation, loudly, and never to "open" and never to a halt.

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

| Day             | Bars        | Of possible | Coverage |
| --------------- | ----------- | ----------- | -------- |
| Monday–Thursday | 1,422–1,436 | 1,440       | ~99.3%   |
| Friday          | 1,018–1,020 | 1,020       | ~99.8%   |
| Sunday          | 405–414     | 420         | ~97%     |

Friday's 1,020 is midnight to 17:00 ET; Sunday's 420 is 17:00 to
midnight. The profile matches the §6.1 boundary to the minute, from a
completely different statistic. This is also the query to re-run once
the store holds a December, per §1.1.

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

The elevated band is 08:00 to 11:00 ET and it drops at 12:00 — which
is exactly the London/New York overlap the rule table predicts
(London 08:00–17:00 BST is 03:00–12:00 ET; New York is 08:00–17:00
ET; the intersection is 08:00–12:00 ET). Peak to trough is **4.5×**.

Two things follow. The overlap-widening rule is confirmed on our own
data rather than adopted on convention. And the 08:00 spike carrying
the 08:30 print is the concrete basis for §1.2 — the hour-of-day
profile already absorbs the routine release.

### 6.4 Absence versus silence

Over one weekend window, Friday 21:00 to Sunday 21:00 UTC:

| Series              | Bars      | Distinct closes | Range     |
| ------------------- | --------- | --------------- | --------- |
| OANDA (both pairs)  | **0**     | —               | —         |
| Twelve Data AUD/USD | **2,880** | 110             | 23.4 bps  |
| Coinbase AUDD/USDC  | 19        | 12              | 130.8 bps |

2,880 is exactly 48 × 60: an **unbroken** minute grid across a market
that was shut. Set against it, a genuinely traded 24/7 crypto tape
produced 19 bars in the same window — because a real venue emits no
candle for a minute with no trades — and moved five times as far.

So weekend coverage is a vendor convention, not a fact about the
market, and the anchor's silence is the trustworthy signal. Session
detection must read bar *absence* from a source that honestly reports
it, never trust a complete grid, and never pool the two into one
statistic.

There is a tempting inference here that this document declines to
make load-bearing. **Grid completeness looks like a synthetic-series
detector** — an unbroken weekend is implausible for a traded market —
and it is a stronger candidate than the `distinct_closes`
collapse heuristic the analytics already tested and rejected as
firing on honest data. But the observation is one vendor over one
weekend. It is recorded as a hypothesis with a stated bar for
adoption: reproduce it across several vendors and several weekends,
including at least one genuinely traded thin tape, before anything
depends on it. Until then, the roster's per-source weekend behavior
is authored knowledge, not detected.

### 6.5 What a wrong calendar costs

Stated concretely, because "the calendar must be right" needs a
blast radius to be actionable. The weekend flag is load-bearing in
three places, and a mis-classification propagates to all of them:

1. **The regime.** Closed-when-open masks a real FX outage as healthy
   (§5.2). Open-when-closed is worse: it degrades the engine on a
   normally-shut market, every week.
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

Three gaps, named rather than papered over:

- **No DST transition is inside the stored window.** The nine
  weekends are all EDT, so the local-time stability is confirmed but
  the *transition* is not directly observed. The ET-local model
  predicts the UTC boundary moves an hour in November; that becomes
  testable then, and it is the single most valuable outstanding check.
- **Christmas and New Year are unobserved** (§1.1).
- **FRED's release-date granularity is unconfirmed**, needing a key
  (§1.3). Immaterial while macro is out of scope.

## 7. Open questions

- **Does the horizon need history at all?** Generated session instants
  are derivable at any time from the rule table, so past rows are a
  convenience for the analytics rather than an irreplaceable record —
  unlike price history, which cannot be reconstructed. If the
  analytics read instants directly, retention matters; if they join
  against them, a rolling window suffices.
- **Do holidays come back as a width input?** Deferred, not refused
  (§1.1). The December re-test decides it.
- **Where does scheduled silence surface to an operator?** §5.1 names
  this as the one piece genuinely still missing. It is a Grafana
  question
  under the standing surface split, and it wants its own scope.
