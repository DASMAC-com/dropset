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

The maker must keep quoting at a **100 bps spread with any one or two
venues dark**. The dashboard shows this *directly*, not by implication:

> **live venues per pair, against the minimum that pair needs.**

Not "sources configured". Not a row count. The number that answers
"can we still quote if OANDA drops right now". A pair at or below its
minimum is the loudest thing on the page.

**CAD/USD was the thin one, and it is the reason the panel exists.**
Measured 2026-09-08: OANDA's v20 instrument list is direction-fixed and
has no `CAD_USD` (only `USD_CAD`, the reciprocal), which left the CADC
anchor on **Twelve Data alone** intraday, against two intraday sources
each for EUR/USD and AUD/USD. Alpha Vantage, Frankfurter and er-api all
carry it, but daily — breadth, not a live-quote input, per §2.

*Closed.* The OANDA adapter now inverts a reversed pair **at intake**:
it fetches `USD_CAD` and flips each candle before the sink, high and
low included, since inverting reverses their order. So CAD/USD has two
intraday sources and meets this section's "any one or two venues dark"
criterion at intraday cadence, like the other two MVP pairs.

The panel is not retired by that, and this episode is the argument for
it: the criterion is met by a **count**, and the count fell to one
without a single feed failing — a mapping fact, invisible to every
staleness bound in §2. So it must still render the per-pair minimum
rather than average it away.

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
is **Kraken, er-api and Pyth** — and it grows, so no panel may assume
it away.

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
anything. Items 6, 8, 10 and 12, and the selector half of item 1,
remain open with no owner yet; each is a *rendering* defect rather than
a missing feed, which is why the feed work did not touch them. (Item 12
was missing from this list before — the omission predates the feeds PR,
and it is exactly the rendering class the sentence describes.)

1. **A tick-only venue is still unreachable in the selector.** The
   *naming* half of this is closed: `Candle rows per minute by source`
   and `Candle coverage` now say which tier they cover, which is the
   second remedy §6.1 allows. What remains open is the selector —
   `var-candle-source` is `SELECT DISTINCT source FROM cex_prices`, so
   a tick-only venue (**Kraken**, **er-api**, **Pyth**) can never be
   *selected* at all. The registry-driven coverage panel is the model
   for the fix: populate selectors from the registry, which knows every
   source by construction. Making the two candle panels read across
   both tables is the fuller remedy and remains open too.

1. **CAD/USD was not collected at all** until 2026-09-07, so §1's
   mandatory anchor was missing for CADC. *Closed, with a caveat worth
   reading.* er-api supplied it first at a daily cadence, which per §2
   is breadth and not a live-quote input; `CAD-USD` is now also on the
   keyed roster (`FX_PRODUCT_IDS`, four pairs), so **Twelve Data**
   carries it intraday and **Alpha Vantage** daily, and Frankfurter
   adds a second daily reference. The anchor is a live-quote input at
   last.

   **OANDA cannot serve it**, which is why the roster there is
   separate — see §3's redundancy note and the comment on the `oanda`
   service. This is the one MVP anchor with a single intraday source.

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
   states 100 bps, which is what sizes its redundancy panel.

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
