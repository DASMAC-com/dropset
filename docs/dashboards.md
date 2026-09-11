<!-- cspell:word delisting -->

# Dropset Dashboards — What They Show and Why

One page, so that the question "what is this dashboard for, and is it
telling me the truth" has a single answer that is not the JSON. The
dashboard JSON stays the source of truth for *rendering*; this document
is the source of truth for *intent*. Where the two disagree, the JSON is
the defect.

Scope: the lean mainnet MVP — **EURC**, **AUDD** and **CADC** against
USDC, our own maker at roughly \$100 top of book. Everything here is
sized to that. See `docs/data-feeds.md` for the ingestion framework and
`docs/market-making.md` for the quoting engine.

## 1. The rule that governs every per-pair panel

Every per-pair panel shows **all three MVP FX pairs**, always:

| Market    | FX pair (the anchor) | Basis (the venue leg)       |
| --------- | -------------------- | --------------------------- |
| EURC/USDC | EUR/USD              | observed — Coinbase, Kraken |
| AUDD/USDC | AUD/USD              | observed — Coinbase         |
| CADC/USDC | CAD/USD              | **assumed 1.0** — see §3.1  |

Two rules follow, and they are deliberately not the same rule:

- **A missing FX pair is a defect.** If a panel cannot show EUR/USD,
  AUD/USD and CAD/USD, the panel is wrong — not the market. There is no
  legitimate state in which an MVP anchor is absent.
- **A pair with no observable basis venue shows the declared
  assumed-peg**, rendered *visually distinct from a faulted source*. An
  assumption that is holding must never render like an outage, and an
  outage must never render like an assumption.

CADC is the whole reason the second rule exists. CADC trades on no
wired venue — verified 2026-09-07 against Coinbase (837 products) and
Kraken (1,446 pairs). Its fair value is therefore the FX anchor times
the 1:1 redemption peg, at a wider spread, per the ratified ruling in
765\. The dashboard states that assumption **in words, on the panel**,
where a basis panel would otherwise be.

## 2. The wired roster

Only these venues. No Chainlink, Band, Supra or new surveys for the
MVP. **Every feed that exists in the stack appears on the dashboard**,
including ones that carry no MVP pair — visibility is the point.

| Venue         | Why it is here                                       | Cadence | Stale after |
| ------------- | ---------------------------------------------------- | ------- | ----------- |
| Coinbase      | The venue leg: real EURC and AUDD basis              | 15 s    | 48 h        |
| Kraken        | Peg truth (USDC/USD, EURC/USD, EURC/EUR) + EURC/USDC | 15 s    | 48 h        |
| OANDA         | The FX anchor; deepest, treated as truth             | 60 s    | 72 h        |
| Twelve Data   | Second independent FX anchor for redundancy          | 60 s    | 72 h        |
| Alpha Vantage | Third FX anchor, daily — a slow cross-check          | 24 h    | 72 h        |
| Frankfurter   | Keyless ECB rates; a composite input                 | 24 h    | 72 h        |
| er-api        | Widest keyless table; sole NGN source                | 24 h    | 72 h        |
| Kraken QCAD   | The §3.1 CAD-stablecoin tripwire, not a peg          | 15 s    | 48 h        |

**Frankfurter is the ECB fix.** One name, always this one; the two are
never wired as separate sources.

**Every row is now wired.** Three rows used to be target state and are
not any more: **Frankfurter**, which had no collector; **Coinbase's
AUDD** leg, silent since 2026-08-17; and **Kraken QCAD**, specified but
uncollected. All three landed together in the feeds PR — §8 items 4, 5
and 9 respectively, each now marked closed there — so the table
describes today rather than an intention. Keeping that distinction
visible matters:
a roster table that quietly mixed the two would be the exact
no-data-reads-as-healthy trap this document exists to close, so if a
row ever goes back to being aspirational, say so here.

**One note on the OANDA row**, and it is a mapping fact rather than an
outage: OANDA's v20 instrument list is direction-fixed and has no
`CAD_USD` (measured 2026-09-08 — only `USD_CAD`, the reciprocal). The
adapter inverts such a pair at intake, so the venue does carry the CADC
anchor — `CAD-USD` is fetched as `USD_CAD` and stored in the canonical
direction, alongside AUD/USD, EUR/USD and GBP/USD. What lands in
`cex_prices` is therefore always canonical, and no consumer needs to
know which rows came from a reversed instrument. See §3.

The 72 h / 48 h split is the class-aware staleness bound already
implemented in `instrument_source_liveness`. Panels **must read that
view** rather than restate the constants, so that two liveness verdicts
cannot silently diverge. Today only the registry-driven coverage panel
does; the rest query the measurement tables directly, which is part of
what §8 item 1 is about.

**The bound is chosen by asset class, so an unseeded currency silently
gets the loosest one.** The venue column above is a summary; what
actually decides is the pair's class, and a pair with an unseeded leg
classes as `unclassified` and takes the fallback. That bit this table's
own QCAD row: `QCAD` was missing from the `currency_kinds` seed, so the
tripwire was held to a 72 h silence rather than the 48 h stated here — it
could have been dark for three days and still read live. Against the
15 s cadence **this table records** for that row, that is four orders of
magnitude of slack; the cadence figure is the table's own claim rather
than a measurement, because no cadence or poll interval is recorded
anywhere in the schema — a fact worth knowing before citing one.

Seeding the leg lands `QCAD-USD` in `peg-pair`, which is the right
class for the right reason rather than merely a tighter number: that
class exists for pairs trading at ~1.0 where only the deviation is
interesting, which is exactly what §3.1 watches. Read this as the
general hazard rather than as one fixed row — **a class-derived bound
means a missing seed is a silent liveness change**, and the roster grows.

### Cadence is not interchangeable with freshness

A daily source is not a slow real-time source; it is a different kind
of claim. Measured 2026-09-07: er-api's CAD/USD read 0.723025 while
Kraken's live USD/CAD of 1.381190 implied a CAD/USD of 0.724013 —
**13.6 bp apart**, entirely
explained by er-api's once-daily snapshot. A daily venue is a breadth
and sanity input. It must never be the sole input to a live quote, and
the dashboard must make its cadence legible so nobody mistakes the one
for the other.

**A gap in a daily series is usually permanent.** The FX collectors'
cursor is an inclusive lower bound at `next_start` — a bar landing
exactly on it is still written — but nothing ever rewinds it. So a bar
that arrives *after* the cursor has passed its timestamp is skipped for
good, and the hole never heals on a later poll. Alpha Vantage carries
two such holes (2026-08-25 and 09-01). Measured on that collector; the
paged-backfill resume cursor is a different thing with different
semantics, so do not read this as a statement about every cursor.
Coverage panels must therefore count
what is *present* over a window rather than infer health from the
latest timestamp, which a permanent hole leaves looking perfectly
fresh.

## 3. Redundancy — the criterion the maker actually needs

The maker quotes at a **100 bps top-of-book spread**, and it quotes a
pair while that pair has **one live trusted intraday tape**, halting at
zero. Those are two separate commitments and only the second one moved:
the 100 bps figure is ratified independently (§8 item 11) and
`docs/market-making.md` is where it is structural — 50 bps each side,
with a worked inventory table. **Do not read the 2026-09-10 criterion
ruling as touching the spread**, which is the mistake this paragraph
exists to prevent: an earlier draft of this section deleted the spread
commitment along with the discredited redundancy clause, on the
reasoning that the ruling had replaced the sentence containing both. It
had not — it replaced one clause of it, and §8 item 11 was left pointing
at a figure §3 no longer stated.

The dashboard shows the criterion *directly*, not by implication:

> **per pair: is a trusted intraday tape live, and how much margin is
> behind it.**

Not "sources configured". Not a row count. Not an average. The number
that answers "can we quote this pair at all right now". A pair at zero
is the loudest thing on the page.

**Ruled by the operator, 2026-09-10, and this section previously stated
the criterion wrongly.** It required quoting "with any one or two venues
dark" and then asserted that CAD/USD's *two* intraday sources satisfied
it — two sources with two dark leaves none, so the section contradicted
itself and no panel could implement both halves. The ruled criterion is
the weaker and more honest one: **one** live trusted tape, halt on zero.

Three parts of the ruling that change how this is read:

- **OANDA is the source believable alone**, per candidate primacy — it
  is §2's anchor, treated as truth. A second intraday source is
  **margin**, not a requirement.
- **A daily reference alone does not clear the bar.** This is §2's
  cadence rule as a quoting gate rather than a display note: three daily
  sources carrying a pair do not make it quotable.
- **Initial mainnet posture is weekday-only quoting**, from the
  operator's laptop. So a weekend with no live tape is the expected
  state rather than a halt to investigate — which is §4's
  weekend-is-not-a-fault rule reaching the quoting path too.

**CAD/USD was the thin one, and it is the reason the panel exists.**
Measured 2026-09-08: OANDA's v20 instrument list is direction-fixed and
has no `CAD_USD` (only `USD_CAD`, the reciprocal), which left the CADC
anchor on **Twelve Data alone** intraday, against two intraday sources
each for EUR/USD and AUD/USD. Alpha Vantage, Frankfurter and er-api all
carry it, but daily — breadth, not a live-quote input, per §2.

*Closed.* The OANDA adapter now inverts a reversed pair **at intake**:
it fetches `USD_CAD` and flips each candle before the sink, high and
low included, since inverting reverses their order. So CAD/USD carries
the trusted tape itself rather than resting on Twelve Data, with a
second intraday source as margin — the same shape as the other two MVP
anchors. Measured 2026-09-10: OANDA's CAD/USD runs about a minute behind
and agrees with Twelve Data to within a few parts in 10^5. **Read that
as the claim, not the levels** — the rate moves, so a quoted pair of
closes would be a spot reading masquerading as a constant, and the
durable fact is the agreement magnitude and the lag. Agreement at that
scale is also what shows the intake inversion is the right way up rather
than off by a reciprocal, which coverage alone cannot tell you: a
reciprocal error still produces a full, fresh, plausible series.

The panel is not retired by that, and this episode is the argument for
it: what fell to one was a **count**, and it fell without a single feed
failing — a mapping fact, invisible to every staleness bound in §2. Under
the ruled criterion the stake is higher rather than lower, because the
margin is what absorbed it: had the inversion not landed, CAD/USD's only
intraday source would have been one the ruling does not consider
believable alone.

**Spreads are stated in bps here and everywhere, never in pips.** A pip
is a fixed absolute increment, so what it is *worth* in relative terms
drifts with the quote level — 1 pip is about 0.855 bp at EUR/USD 1.17
and about 0.952 bp at 1.05. A maker quoting a relative spread would
have its target silently move with the market, which is the wrong
behavior; bps is level-invariant and is what
`docs/market-making.md` commits to.

### 3.1 The CAD-stablecoin tripwire

CADC quotes off the FX anchor times an assumed 1.0 peg, at roughly
**30 bps**. An assumption nobody watches is a liability, so one panel
watches it: **QCAD/USD against the CAD/USD composite, with the
threshold drawn on it.**

QCAD is a *different issuer's* Canadian stablecoin, which is precisely
what makes it useful and precisely why it can never be the peg. It is a
**class-level** check: if Canadian stablecoins generally drift off par,
the assumption underneath CADC is no longer safe, whoever the issuer
is. Divergence past a coarse **1–2%** band marks the CAD-stablecoin
class suspect, which widens or halts CADC.

Coarse on purpose. QCAD traded 14 times in the 24 h to 2026-09-07, so a
tight band would fire on its own thinness; measured that day it sat
4.0 bp rich to the CAD rate, well inside the band. This is a tripwire
for a regime change, not a pricing input, and it cost one product-id
line on the Kraken collector already in the roster — no new venue.
`QCAD-USD` is now on that roster.

## 4. Healthy, faulted, parked

Three states, three distinct renderings. Conflating any two of them is
the failure mode this dashboard exists to prevent.

- **Healthy** — producing inside its class staleness bound.
- **Faulted** — expected to produce and not producing. Loud.
- **Parked** — deliberately not producing. Quiet, legible, never red.

Parked is a real state with real occupants, and it must be visibly
*chosen*: Pyth is dark by decision — Hermes now requires a Bearer key,
and we hold none, which is why it stays parked. Note that is the reason
it is *not restarted*, not a diagnosis of the original outage: our
collector's failure was never pinned to a status code, because the log
line omits one. CADC's
basis is an assumed peg, not a broken feed. A parked source rendered as
a fault trains the operator to ignore red, which costs more than the
outage it was meant to surface.

**Parked means not running.** A parked venue whose container is still
up is not parked — it is a fault wearing the label, and it burns
requests against a venue that has already refused us. Pyth is gated
behind a compose profile precisely so that it stays stopped, and on
2026-09-07 a stray start had it retrying an erroring host every five
seconds while the board recorded it as parked. The dashboard must be
able to tell those two apart, because the whole value of the parked
state is that it is quiet, and a quiet fault is the worst thing this
page can render.

**The parked set is declared in code**, as `PARKED_SOURCES` in
`feeds/src/parked.rs`: one entry per parked source, carrying the date it
has been parked since and the reason. It is keyed on the bare venue
token (`pyth`), not the framework feed name (`pyth-hermes`) that
`feed_health` records. It is a Rust constant rather than a column on
`instrument_registry` because that table is written only by a *running*
collector, through `register_instruments` — so a parked source can never
write the row that would say it is parked. The maker bot reads the set at
its spawn site and does not start a parked tier, which makes the
*not-running* half of the rule above hold by construction rather than by
operator discipline. `feeds/tests/parked_compose_agreement.rs` pins each
entry against the deployment: the service must sit behind a compose
profile, that profile must not be the one `collectors-up` enables, and
the Makefile's start lists must not name it. A deliberate opt-in start
(`make pyth-up`) is still possible and is what a park is for; what the
test forbids is starting as a side effect of the ordinary bring-up.

**What this page cannot do with it yet.** Because the set lives in code,
no panel query can join against it: separating parked from faulted here
needs either a copy of the list inside the query or a later change
seeding it into reference data. The marker changes nothing about what the
existing coverage query returns — that query reads `instrument_registry`,
which a parked source never wrote in the first place. It does change
`feed_health`: parking removes the tier's only writer, so its row stops
being updated, and in a fresh database it never appears at all. A parked
source therefore renders on the feed-health panel and the staleness alert
exactly as the "invisible, not dark" hazard below describes — absent, or
frozen at its last value — never labelled parked. Worse for a row written
before the park: it keeps `last_ok_at` NULL, so the unfiltered
`ok_age_secs > 1800` alert goes on firing with nothing left running that
could clear it. Before the park a credential arriving would have cleared
it; now only an exclusion on the alert or a one-off delete will.

**A weekend is not a fault either.** Alpha Vantage produces weekday
daily bars, so on any Sunday it is correctly silent while reading dark
against a wall-clock bound. Sessions come from the market calendar
(`docs/market-calendar.md`), which is the single DST authority.

## 5. What each panel means

**About ten panels on the market-data dashboard. This is a hard
constraint, not a target.** Breadth belongs in the drop-downs, not in
more panels: pick the pair, the source, the granularity, and the same
ten panels answer the question. A dashboard nobody can take in at once
does not get read, and an unread dashboard is worse than none, because
it is trusted without being looked at.

Panel *count* is not the whole of it, and it is the half that misleads.
A repeating panel is one panel in the JSON and N charts on the screen,
so the constraint is on **rendered charts**: a panel that repeats over
a variable defaulted to All silently breaks this rule while the JSON
still looks compliant. Repeat over a deliberately narrow default —
never over All — and let the drop-down widen it on demand.

One sentence each; if a panel needs more, the panel is doing two jobs.

- **Source coverage (registry-driven)** — every source the collectors
  registered, whether or not it is producing, so a dark collector reads
  0 rather than vanishing.
- **Live venues per pair** — §3's redundancy criterion.
- **Price by source** — the same pair from every venue that carries it;
  divergence between venues is the signal.
- **Fair price over its sources** — what the maker will actually quote
  from, and which inputs composed it.
- **Feed health / staleness** — age against the class bound, per
  source and product.
- **Candle rows per minute by source** — cadence actually observed,
  against the cadence claimed in §2. Candles only; the tick tier is a
  separate panel, and the name has to say so.
- **OHLC candles** — price action for one product at one granularity.
- **Feed cursor age** — how far behind its own watermark a feed is.

## 6. Two tiers, deliberately

- **MVP pairs** (EUR/USD, AUD/USD, CAD/USD) get the full per-pair
  treatment: every panel above, per pair.
- **Thin-roster currencies** get **at least source visibility** — what
  powers them and how stale it is. The localnet demo maker keeps
  quoting the full roster; only the engine depth differs. A roster
  currency with no visible source is a defect at this tier too, even
  though it gets no per-pair panels.

### 6.1 A tick-only venue must not be able to vanish

Sources land in **two tiers by storage**, and this is the seam every
visibility bug so far has fallen through. Candle venues write
`cex_prices`; tick venues write `spot_ticks`. Today the tick-only class
is **Kraken, er-api, Frankfurter and Pyth** — and it grows, so no panel
may assume it away.

**Frankfurter joined that class when it was wired, and this sentence
lagged it** — the "it grows" prediction coming true, found by reading the
store rather than by reasoning. Note which way the drift ran: the
`Source` variable's own description already named Frankfurter, so the
JSON was right and the prose was the stale side. That is the expected
direction, because a panel is exercised every time someone loads the
dashboard and a paragraph never is, and it is the reason to check this
list against the store rather than against memory. `coinbase` is the one
source in **both** tiers, so the tier is a property of the row's table,
never of the source label.

The rule: **a panel whose name says "by source" must cover both
tables, or its name must say which tier it covers.** A query over
`cex_prices` alone is not wrong, but calling its output "every source"
is, and it fails silently — the venue does not error, it is simply
absent, which is indistinguishable from not existing.

The same applies to any variable that *populates from* a measurement
table. A source drop-down built as `DISTINCT source FROM cex_prices`
can never offer a tick-only venue, so the venue becomes unreachable
even on panels that would happily show it. Populate selectors from the
registry, which knows every source by construction.

## 7. What is deliberately not shown

Naming these stops them being re-litigated every time someone notices
one missing.

- **Order-book depth and fills.** The maker's own surface; this is an
  ingestion dashboard. Writes and command safety live in the TUI.
- **Aerodrome (or any DEX) CADC price.** Aerodrome carries most real
  CADC volume, so it is where genuine CADC price discovery happens.
  **Rejected for the MVP** — it would be a new venue, and the tripwire
  in §3.1 covers the risk it would have addressed.
- **QCAD as a price.** QCAD is a *different issuer's* Canadian
  stablecoin, so it can never be CADC's peg. It appears only as the
  §3.1 tripwire.
- **Per-request logs and CU counts.** Not observability; noise.
- **Anything Pyth-shaped** beyond its parked row, while it stays
  parked.

## 8. Known gaps — the punch list this spec produced

Recorded here because a spec that hides its own unmet requirements is
worse than no spec. Each is a defect against a rule above.

**What closed here, and what did not.** The PR carrying this spec closed
item 3 outright and the naming half of item 1. The four roster changes
— items 2, 5, 7 and 9 — and item 4, the Frankfurter collector, landed
together in the following feeds PR, deliberately, so the dashboard is
arranged once against a complete feed set rather than twice. That PR
also settled item 11, on an operator ruling rather than by building
anything. Items 6, 8, 10 and 12 remain open with no owner yet; each is a
*rendering* defect rather than a missing feed, which is why the feed work
did not touch them. (Item 12 was missing from this list before — the
omission predates the feeds PR, and it is exactly the rendering class the
sentence describes.)

**The feed-verification PR added items 13 to 17 and retired item 1.**
Named for what it did rather than for the issue it sits under: the
attended walkthrough that issue is titled for was deliberately deferred
to a follow-up, so do not read these items as walkthrough results. It
closed
13 by building the panel and 2's QCAD half by seeding the currency kind;
14 and 15 record what building 13 taught, one of them a defect in this
document's own arithmetic. Items 16 and 17 are held deliberately — they
are inputs to the per-panel adjudication rather than work to do — and 17
is where the ~10 chart cap gets resolved. Two of these came from
*reading the store and asking whether the spec's claims were true*,
rather than from checking the JSON against the spec: item 1 was a fixed
defect still listed as open, and item 13 a requirement no item mentioned.
Both directions of drift are real, so both checks are worth running.

1. **A tick-only venue is still unreachable in the selector.** *This
   item was overtaken and its remaining half was mis-stated; read the
   correction rather than the claim.* The *naming* half closed as
   recorded: `Candle rows per minute by source` and `Candle coverage`
   say which tier they cover, the second remedy §6.1 allows.

   The selector half is **not** open in the form written here. `Source`
   — the multi-select the cross-tier panels read — is already
   `SELECT DISTINCT source FROM instrument_registry`, so a tick-only
   venue *is* selectable, and `Price by source` unions both tables and
   shows it. What remains measurement-derived is `var-candle-source`
   alone (`SELECT DISTINCT source FROM cex_prices`), and that one is
   **deliberate rather than defective**: it feeds only the OHLC panel,
   which reads `cex_prices`, so offering Kraken there would buy a
   guaranteed-empty chart — trading an unreachable venue for an
   ambiguous blank, which §4 likes even less.

   Two things worth keeping from the error, since the wrong half is the
   instructive one. The claim was *verified against the store rather
   than re-read from the JSON*, which is how a fixed defect stayed on a
   punch list. And "populate selectors from the registry" is a rule
   about **selectors whose panels can show every tier**, not a rule
   about every selector; stated without that bound it argues for a
   change that makes the dashboard worse.

1. **CAD/USD was not collected at all** until 2026-09-07, so §1's
   mandatory anchor was missing for CADC. *Closed, with a caveat worth
   reading.* er-api supplied it first at a daily cadence, which per §2
   is breadth and not a live-quote input; `CAD-USD` is now also on the
   keyed roster (`FX_PRODUCT_IDS`, four pairs), so **Twelve Data**
   carries it intraday and **Alpha Vantage** daily, and Frankfurter
   adds a second daily reference. The anchor is a live-quote input at
   last.

   **OANDA now serves it too**, via intake inversion — it quotes only
   `USD_CAD`, which the adapter fetches and flips, so the pair has a
   second intraday source and is no longer the one MVP anchor short of
   the §3 criterion. OANDA's roster stays a separate variable even so,
   for a reason that outlives the mechanism: see §3's redundancy note
   and the comment on the `oanda` service.

1. **er-api had never run — fixed 2026-09-07.** It was wired into the
   compose file but reached neither place that makes a collector run:
   no Makefile target listed it, and the collectors image never copied
   its binary, so it failed at start with
   `executable file not found in $PATH`. Both are now corrected. It is
   the only keyless CAD/USD and the only NGN source, and starting it
   added 14 live source-product pairs, 10 to 24. The observed total that
   evening was 25: Alpha Vantage published its Monday bar during the
   same window, which is unrelated to er-api and is counted separately
   here so the 14 stays attributable.

1. **Frankfurter has no market-data collector.** *Closed.* Ratified as
   a real price feed and an input to the composite at its honest daily
   cadence, so this was required work, not a cut. `market-data-frankfurter`
   now exists, with the Makefile target and image COPY line er-api
   needed. The adapter's shape did **not** match er-api's after all:
   er-api yields a struct carrying the provider's refresh instant while
   Frankfurter yielded a bare `Quotes` map, so the copy needed a new
   snapshot type before it would fit — the collector binary itself is
   close to er-api's, guard shape included. The response's `date` field
   is now plumbed through an
   additive snapshot type and each reading is stamped at **midnight UTC
   of the ECB reference date** rather than the poll second — without
   which a Friday fix would read as fresh on Sunday night. The
   maker's cascade still consumes the bare map, unchanged.

1. **AUDD/USDC stopped on 2026-08-17** and was then de-rostered by
   config. *Closed.* Coinbase still lists it `online` with trading
   enabled — re-verified 2026-09-08 — so this was never a delisting,
   and both collectors now default to `EURC-USDC,AUDD-USDC`.

   Two honest limits on that. The order was venue-first: it went quiet,
   *then* we de-rostered, so the original silence was not ours — what
   was ours is that it became invisible rather than dark (item 6).
   And re-rostering is verified on the **ticker** leg only, which
   returned a print at 0.71805; the **candle** leg is asserted, not
   measured, because candles come from trades and the pair is thinly
   traded. Expect its `cex_prices` series to stay empty until it trades
   again — a true-and-expected blank, which §4 still cannot render
   distinctly (item 10).

1. **A de-rostered product is invisible, not dark.** The registry is
   written at collector start, so dropping a product from the roster
   removes it from the coverage panel entirely — the exact defect that
   panel exists to prevent, one level up. **AUDD was the live instance
   and is no longer**, having been re-rostered when item 5 closed. The
   defect stands with nothing currently demonstrating it, which makes it
   easier to forget and no less real: the next de-rostering reproduces
   it silently.

1. **Kraken lists EURC/USDC directly** and we did not collect it.
   *Closed* — `EURC-USDC` is on the Kraken roster. It is free
   redundancy on the exact MVP pair, against §3's criterion, and the
   poll is batched so it costs no extra request.

1. **A crash-looping collector is invisible after bring-up**, which is
   the standing issue 1128 and now has a live instance to point at: on
   2026-09-07 the parked Pyth collector was found retrying an erroring
   host every five seconds, having been started outside any bring-up
   target, with nothing on any dashboard saying so. The §4
   parked-versus-faulted rendering is the front half of that fix — it
   is what makes the state legible; the back half is noticing the
   process at all, which no panel does today.

1. **The §3.1 QCAD tripwire is specified but not collected.** *Closed* —
   `QCAD-USD` is on the Kraken roster, so §2's row for it describes a
   feed rather than a target. Kraken keys the pair `QCADUSD`, which
   plain concatenation derives, so it needed no pinned spelling.

1. **CAD/USD had no candles from any source**, only er-api ticks, so the
   OHLC panel's CAD/USD repeat was empty by construction until item 2
   landed. **Item 2 has landed**, so the emptiness itself is resolved:
   **Twelve Data** now carries `CAD-USD` intraday and **Alpha Vantage**
   daily, and both write candles into `cex_prices`. **OANDA too**, now
   that the adapter inverts a reversed pair at intake and serves
   `CAD-USD` via `USD_CAD` (see §3) — so candle coverage here matches
   the other two MVP anchors rather than trailing them.
   The tick side gained a source too: Frankfurter now writes CAD/USD
   alongside er-api. What this item was *really* about is untouched and
   stays open — §4 still has no rendering that distinguishes "true and
   expected" from "faulted", so any genuinely-empty repeat reads as the
   same ambiguous blank as the Fusion-weight panel below. The live
   example moved; the defect did not.

1. **The spread figure disagrees with `docs/market-making.md`.**
   *Closed by operator ruling, 2026-09-08.* **100 bps stands** as the
   MVP top-of-book spread: `docs/market-making.md` is current and its
   figure is structural there (50 bps each side, plus a worked
   inventory table), while §3's lean figure was the stale side. §3 now
   states 100 bps.

   **The clause that used to close this item — "which is what sizes its
   redundancy panel" — is retired**, and it is worth saying why rather
   than deleting it silently. It was true only under the
   one-or-two-venues-dark reading, where the spread and the redundancy
   count were one requirement; the 2026-09-10 ruling separated them, and
   the shipped panel contains no bps and nothing derived from the spread.
   Since §3 sends readers here as the spread figure's authority, leaving
   the clause would have re-coupled the two things §3 now tells them to
   keep apart.

   The same ruling settled the **unit**: bps everywhere, never pips —
   see the note in §3 for why a level-drifting increment is the wrong
   denomination for a maker's relative spread. A tree-wide sweep found
   pips in exactly three places: §3's two *spread figures*, both now
   bps, and a bps→pip conversion in
   `docs/research/crypto-oracle-survey.md` illustrating a third party's
   oracle threshold, which is bps-primary already and was left alone.
   Note §3 still mentions pips — the ruling bans them as a
   **denomination**, not as a word, and the note explaining why quotes
   the conversion it rejects.

1. **A panel can render an ambiguous blank.** `Fusion weight by source`
   reads `maker_leg_contributions`, which is empty whenever no maker is
   running — the ordinary state on a collectors-only stack. It renders
   "No data", indistinguishable from a broken query. This is §4's rule
   with nothing implementing it, and it is the generalization of the
   item above.

1. **§3's redundancy panel did not exist**, and no item on this list
   said so. *Closed — `Live venues per pair` is now built.* §3 claimed
   the dashboard shows the criterion "directly, not by implication" and
   §5 listed the panel; neither was true, and the gap survived twelve
   punch-list items because every *individual* panel matched its own
   description. What no reading of the JSON catches is a panel that is
   absent, so the check has to run the other way — from the spec's
   claims to the JSON — which is the lens this list was missing.

1. **Pooling cadences would have overstated redundancy 2.5×.** Recorded
   because the wrong version was built first and the live data caught
   it. Every FX anchor reads five live sources and only **two** are
   intraday, so the pooled count answers "how many venues carry this
   pair" when §3 asks "how many could quote it right now".

   The first fix split the two on **recency**, which is wrong in a way
   that reads as right: a daily venue is genuinely fresh for the hour
   after it publishes, so er-api was promoted into the quotable count
   once a day — the exact cadence-versus-freshness conflation of §2,
   reintroduced by the panel built to respect it.

   The second fix counted readings over a six-hour window, and **that
   one was worse, because it failed open.** A genuinely intraday venue
   down longer than the window has no readings in it, so it stopped
   counting as intraday and moved into the *daily* population — while
   `instrument_source_liveness` still called it live, that bound being
   48–72 h. The panel therefore looked **healthier the longer the outage
   ran**, which is the one direction a liveness panel must never fail.
   Verified against the store: a source can be `is_live` with zero
   readings in six hours.

   The shipped version has **no cadence classifier at all**. Under the
   ruled criterion it needs none — cadence only ever mattered as a proxy
   for "could this price a quote right now", and a *designated* tape plus
   a recency test answers that directly and fails closed. The general
   lesson is the one that survives the specific panel: **a classifier
   built from the same signal an outage suppresses will always
   misclassify the outage.** Both wrong versions read as careful; only
   running them against a store that had a 24-hour-old live source
   distinguished them.

1. **§3's own arithmetic did not add up.** *Closed by operator ruling,
   2026-09-10.* §3 required quoting with "any one or two venues dark"
   and then asserted CAD/USD's **two** intraday sources met it — two
   sources with two dark leaves none, so the section contradicted itself
   and no panel could implement both halves. The ruled criterion is
   **one live trusted intraday tape, halt on zero**, with a second
   intraday source as margin; a daily reference alone does not clear it.
   §3 now states that.

   Worth keeping about the shape of this one: it was found by *building
   the panel*, not by reading the section. The contradiction had sat in
   prose through several revisions because prose can hold both halves
   comfortably — it is only when something has to compute a threshold
   that the two stop being compatible. An unimplementable requirement
   reads as a fine requirement until someone implements it.

1. **Two panels the operator could not parse.** Recorded as observed,
   not diagnosed, and deliberately not redesigned here — they are inputs
   to the commissioned per-panel adjudication, which will rule on
   keep/cut/merge/rename against the question each panel answers.

   - `Feed cursor age (wall clock)` renders as 12–15 small unlabeled
     boxes with no way to tell which feed each one is.
   - `Fair price over its sources` versus `Fusion weight by source`
     reads as an unclear distinction between the two.

1. **The rendered-chart budget is already over, before the reserved
   panels land.** §5's "about ten, a hard constraint" was 12 before this
   PR: ten panels, of which nine render one chart each and the OHLC panel
   repeats over the three MVP anchors (9 + 3). The redundancy panel added
   here makes eleven panels and 13 rendered charts. The pricing-path work
   reserves three more readings, all of which read an estimator table that
   does not exist on `main` yet. Note the two rules are in genuine
   tension rather than merely unmet: §1 requires every per-pair panel to
   show all three anchors, so the repeat that breaches the cap is the
   same rule that §1 mandates. Resolving that is the adjudication's job,
   which is why the layout iteration is held rather than done here.
