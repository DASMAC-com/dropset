<!-- cspell:word QCAD -->

<!-- cspell:word rostered -->

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

| Venue         | Why it is here                              | Cadence | Stale after |
| ------------- | ------------------------------------------- | ------- | ----------- |
| Coinbase      | The venue leg: real EURC and AUDD basis     | 15 s    | 48 h        |
| Kraken        | Peg truth (EURC/EUR, EURC/USD, USDC/USD)    | 15 s    | 48 h        |
| OANDA         | The FX anchor; deepest, treated as truth    | 60 s    | 72 h        |
| Twelve Data   | Second independent FX anchor for redundancy | 60 s    | 72 h        |
| Alpha Vantage | Third FX anchor, daily — a slow cross-check | 24 h    | 72 h        |
| Frankfurter   | Keyless ECB rates; a composite input        | 24 h    | 72 h        |
| er-api        | Widest keyless table; sole NGN source       | 24 h    | 72 h        |
| Kraken QCAD   | The §3.1 CAD-stablecoin tripwire, not a peg | 15 s    | 48 h        |

**Frankfurter is the ECB fix.** One name, always this one; the two are
never wired as separate sources.

The 72 h / 48 h split is the class-aware staleness bound already
implemented in `instrument_source_liveness`. Panels **read that view**
rather than restating the constants, so a liveness verdict can never
silently diverge between two places.

### Cadence is not interchangeable with freshness

A daily source is not a slow real-time source; it is a different kind
of claim. Measured 2026-09-07: er-api's CAD/USD read 0.723025 while
Kraken's live USD/CAD implied 0.724013 — **13.6 bp apart**, entirely
explained by er-api's once-daily snapshot. A daily venue is a breadth
and sanity input. It must never be the sole input to a live quote, and
the dashboard must make its cadence legible so nobody mistakes the one
for the other.

**A gap in a daily series is usually permanent.** A feed cursor is an
inclusive lower bound at `next_start` — a bar landing exactly on it is
still written — but nothing ever rewinds it. So a bar that arrives
*after* the cursor has passed its timestamp is skipped for good, and
the hole never heals on a later poll. Alpha Vantage carries two such
holes (2026-08-25 and 09-01). Coverage panels must therefore count
what is *present* over a window rather than infer health from the
latest timestamp, which a permanent hole leaves looking perfectly
fresh.

## 3. Redundancy — the criterion the maker actually needs

The maker must keep quoting at a **20–30 pip spread with any one or two
venues dark**. The dashboard shows this *directly*, not by implication:

> **live venues per pair, against the minimum that pair needs.**

Not "sources configured". Not a row count. The number that answers
"can we still quote if OANDA drops right now". A pair at or below its
minimum is the loudest thing on the page.

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
for a regime change, not a pricing input, and it costs one product-id
line on the Kraken collector already in the roster — no new venue.

## 4. Healthy, faulted, parked

Three states, three distinct renderings. Conflating any two of them is
the failure mode this dashboard exists to prevent.

- **Healthy** — producing inside its class staleness bound.
- **Faulted** — expected to produce and not producing. Loud.
- **Parked** — deliberately not producing. Quiet, legible, never red.

Parked is a real state with real occupants, and it must be visibly
*chosen*: Pyth is dark by decision (keyed since 2026-08-26); CADC's
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

1. **A tick-only venue is invisible on the "by source" panels.** This
   is the one that bites now. `rows per minute by source` and
   `source × product coverage` both query `cex_prices` alone, so every
   venue that writes only `spot_ticks` — **Kraken**, **er-api**,
   **Pyth** — is missing from panels whose names promise every source.
   `var-candle-source` compounds it: it is
   `SELECT DISTINCT source FROM cex_prices`, so a tick-only venue can
   never even be *selected*. Kraken has been live and fresh throughout
   and still reads as absent, which is exactly the every-wired-feed
   rule failing. The registry-driven coverage panel is the one that
   gets this right, and is the model for the fix — these panels must
   read across both tables, not one.
1. **CAD/USD was not collected at all** until 2026-09-07, so §1's
   mandatory anchor was missing for CADC. Now supplied by er-api at a
   daily cadence, which per §2 is breadth and not a live-quote input:
   the keyed roster (`FX_PRODUCT_IDS`) is the three pairs those vendors
   are paid for, and adding CAD/USD there is what closes this properly.
1. **er-api had never run — fixed 2026-09-07.** It was wired into the
   compose file but reached neither place that makes a collector run:
   no Makefile target listed it, and the collectors image never copied
   its binary, so it failed at start with
   `executable file not found in $PATH`. Both are now corrected. It is
   the only keyless CAD/USD and the only NGN source, and starting it
   took live source-product pairs from 10 to 24.
1. **Frankfurter has no market-data collector.** Ratified as a real
   price feed and an input to the composite at its honest daily
   cadence, so this is required work, not a cut. It exists today only
   as a feeds-crate venue adapter the demo maker consumes live: there
   is no `market-data-frankfurter` binary, so unlike er-api this is new
   code rather than wiring — the adapter's shape matches er-api's
   source, so the collector is close to a copy, plus the same Makefile
   and image COPY lines er-api needed.
1. **AUDD/USDC stopped on 2026-08-17** and is de-rostered by config
   (`PRODUCT_IDS` defaults to `EURC-USDC`). Coinbase still lists it
   `online` with trading enabled, so this is a roster change, not a
   delisting.
1. **A de-rostered product is invisible, not dark.** The registry is
   written at collector start, so dropping a product from the roster
   removes it from the coverage panel entirely — the exact defect that
   panel exists to prevent, one level up. AUDD is the live instance.
1. **Kraken lists EURC/USDC directly** and we do not collect it. It is
   free redundancy on the exact MVP pair, against §3's criterion.
1. **A crash-looping collector is invisible after bring-up**, which is
   the standing issue 1128 and now has a live instance to point at: on
   2026-09-07 the parked Pyth collector was found retrying an erroring
   host every five seconds, having been started outside any bring-up
   target, with nothing on any dashboard saying so. The §4
   parked-versus-faulted rendering is the front half of that fix — it
   is what makes the state legible; the back half is noticing the
   process at all, which no panel does today.
