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
| CADC/USDC | CAD/USD              | **assumed 1.0** — no venue  |

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
| Frankfurter   | Keyless ECB breadth; powers the demo maker  | 24 h    | 72 h        |
| er-api        | Widest keyless table; sole NGN source       | 24 h    | 72 h        |

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

## 3. Redundancy — the criterion the maker actually needs

The maker must keep quoting at a **20–30 pip spread with any one or two
venues dark**. The dashboard shows this *directly*, not by implication:

> **live venues per pair, against the minimum that pair needs.**

Not "sources configured". Not a row count. The number that answers
"can we still quote if OANDA drops right now". A pair at or below its
minimum is the loudest thing on the page.

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

**A weekend is not a fault either.** Alpha Vantage produces weekday
daily bars, so on any Sunday it is correctly silent while reading dark
against a wall-clock bound. Sessions come from the market calendar
(`docs/market-calendar.md`), which is the single DST authority.

## 5. What each panel means

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
- **Rows per minute by source** — cadence actually observed, against
  the cadence claimed in §2.
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

## 7. What is deliberately not shown

Naming these stops them being re-litigated every time someone notices
one missing.

- **Order-book depth and fills.** The maker's own surface; this is an
  ingestion dashboard. Writes and command safety live in the TUI.
- **Aerodrome (or any DEX) CADC price.** Aerodrome carries most real
  CADC volume, so this is where genuine price discovery happens — and
  it is a post-MVP candidate basis venue, recorded on 765. Excluded
  now only because the MVP wires no new venues.
- **QCAD as a price.** QCAD is a *different issuer's* Canadian
  stablecoin, so it can never be CADC's peg. It may appear as a
  wide-band **sanity check** that Canadian stablecoins are near par —
  measured 2026-09-07 at 4.0 bp rich to the CAD rate, on 14 trades a
  day. A sanity check, never an input.
- **Per-request logs and CU counts.** Not observability; noise.
- **Anything Pyth-shaped** beyond its parked row, while it stays
  parked.

## 8. Known gaps — the punch list this spec produced

Recorded here because a spec that hides its own unmet requirements is
worse than no spec. Each is a defect against a rule above.

1. **CAD/USD is not collected at all**, so §1's mandatory anchor is
   missing for CADC. The keyed roster (`FX_PRODUCT_IDS`) is the three
   pairs those vendors are paid for and does not include it.
1. **er-api has never run.** It is wired into the compose file but
   appears **nowhere in the Makefile**, so no target starts it — while
   its own comment claims it "comes up with `make collectors-up`". It
   is the only keyless CAD/USD and the only NGN source.
1. **Frankfurter has no market-data collector.** It exists as a feeds
   venue the demo maker consumes live, so it writes nothing to the
   store and cannot appear on any panel. Specified here, not cut.
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
