<!-- cspell:word guéant -->

<!-- cspell:word illiquidity -->

<!-- cspell:word lehalle -->

<!-- cspell:word parameterizes -->

<!-- cspell:word raydium -->

<!-- cspell:word stoikov -->

<!-- cspell:word tapia -->

# Market Making — multi-market FX stablecoins

The **operating spec** for Dropset's market-making vaults: how a single
leader bot quotes a roster of non-USD FX stablecoins against USDC on the
eCLOB, each at a 100 bps spread with \$100 of top-of-book inventory per
side. It pins down the four numbers a bot needs and nothing more —
reference-price construction, the `LiquidityProfile` ladder, update
cadence, and the inventory/kill-switch policy. The math is anchored to
Avellaneda–Stoikov but the ladder is hand-shaped: stable-pair σ is too
small for the formal A-S skew to matter at this size, so the bot uses A-S
as a sanity check and a linear override for inventory.

## Status

**The localnet milestone is met.** `make demo` stands the whole roster up
end to end: the TUI control plane brings up the validator, the explorer
stack, and the markets, then launches a leader that quotes every FX pair
on the eCLOB with a benign flow taker moving the books — with the
frontend routing against the same chain. What this document specifies is
therefore no longer a design to be proven; it is the **operating spec of
a running maker**, and the work in front of it is hardening that same
maker toward production rather than building a second one.

What production requires **on top** of the demonstrated behavior, each
tracked as its own effort under the maker-hardening umbrella:

- **Intraday feeds** — the standing market-data store behind the FX and
  basis legs (`data-feeds.md`), so fair value is driven by a recorded,
  queryable history rather than whatever a process happened to poll.
- **Telemetry** — per-feed health and quote/fill observability surfaced
  where an operator sees it, not only in log lines.
- **Parameter channel** — changing a live bot's knobs (bands, floors,
  ladder shape) without a redeploy.
- **Volatility-driven ladder** — σ and per-level TIF derived from
  measured volatility instead of the hand-tuned constants in §2.
- **Deploy apparatus** — the devnet/mainnet promotion path, delegated
  `quote_authority` hot keys, and secret handling.

Three principles hold across all of it, and any change to this spec is
read against them:

1. **One maker crate.** There is no demo fork and no demo-only code path.
   The bot that quotes on localnet is the bot that quotes on mainnet;
   environments differ by configuration, not by build.
1. **The demo flies the production stack.** `make demo` exercises the
   real components in the real arrangement — test like you fly, fly like
   you test. A thing that only works under `make demo` is not done.
1. **Quoting never depends on Postgres.** The store is where market data
   accumulates, not a link in the quoting loop: the maker must keep
   quoting, or halt deliberately per §4, when the database is unreachable.
   Postgres is a soft dependency of the bot, always.

**Doc boundary.** Dependency flows one way: **`market-making.md` →
`architecture.md`**. This document references *down* into the protocol
spec (`LiquidityProfile`, `SetReferencePrice`, `SetLiquidityProfile`,
`FreezeVault`, flush math) and never the other way around. Nothing in
the protocol depends on this strategy — a different leader can run a
different shape against the same instructions.

**Objective.** Breadth, not yield. The maker flashes credible
top-of-book across many FX stablecoins at once, all routed through one
SDK — the illiquidity story is that these markets have no liquid home on
Solana, and Dropset's maker stack can stand one up on demand. The roster
exercises the leader interfaces end to end (price/profile cadence,
inventory drift, peg-deviation alarms) across every market at once.

**Scope.**

- Markets: seven `<token>/USDC` pairs — EURC (EUR), VCHF (CHF), TGBP
  (GBP), ZARP (ZAR), MXNe (MXN), XSGD (SGD), IDRX (IDR). Each token
  tracks its fiat with a peg discount that is usually small but not zero.
  The roster spans orders of magnitude in unit price (EURC ~\$1.14 down
  to IDRX ~\$0.000056), so decimals and the `Price` encoding are handled
  per market (see **Per-market decimals** below).
- Per-vault inventory: **\$100 of top-of-book per side** at launch
  (~\$100 base + ~\$100 USDC, balanced at the seed reference). The full
  leg sits at the top level, so the seeded book flashes ~\$100 a side.
  Per-market TVL-floor / skew calibration is coordinated separately.
- Spread target: **100 bps quoted** at top of book (50 bps each side of
  mid). Holds for ~\$20 of one-sided trade; wider beyond.
- Single leader across all markets. No hedging, no shorting, no leverage.
  The delegated per-market `quote_authority` model is the devnet/mainnet
  promotion's concern.

**Per-market decimals.** The feeds report a **human** quote-per-base
price (USD per token); the engine stores the **atoms-ratio**
(`quote_atoms` per `base_atoms`) the on-chain `Price` encodes. They
coincide only when both legs share decimals — so the bot scales the
human price by `10^(quote_decimals − base_decimals)` at the chain write
boundary. `Price` is a `u32` with 8 significant digits and a base-10
exponent spanning ~`1e-16 … 1e16`, so the whole roster (IDRX's
~\$0.000056 included) encodes with full precision.

______________________________________________________________________

## 1. Reference-price construction

Fair value is a **fast, deep, exogenous FX driver corrected by a slow,
thin stablecoin basis**:

```text
fair = fx_rate × basis
```

- `fx_rate` is the fiat cross (EUR/USD for the EURC market, GBP/USD for
  TGBP, …) — the **anchor**. It is deep, continuously priced on
  interbank / CME venues, and exogenous to Dropset: it does not move
  because we quote.
- `basis` is the token's **peg discount** against that fiat, a
  multiplicative correction near 1. Correcting the anchor is the token
  price's only job — it is slow and thin, so it is smoothed, not chased.

This inverts the earlier cascade, which made the token's crypto/USD price
(CoinGecko) the primary mid and FX a degraded fallback. That is backwards
for the pricing edge: the crypto/USD feed is laggy and *reflexive* (it is
derived in part from the very venue prints it is meant to correct), so
anchoring on it makes the bot lag exactly when the edge appears.

### Two-peg decomposition

`basis` is not one number — USDC is **not** assumed equal to USD. Each
market quotes `<token>/USDC`, so the basis carries **both** pegs:

```text
basis = (token / fiat) ÷ (USDC / USD)
```

Worked for EURC/USDC: `basis = (EURC/EUR) ÷ (USDC/USD)`, so
`fair = (EUR/USD) × (EURC/EUR) ÷ (USDC/USD)` — the EUR/USD anchor scaled
by how EURC trades against EUR and how USDC trades against USD. Both peg
legs are first-class inputs; collapsing `USDC/USD → 1` hides a correlated
risk (failure mode 1).

### Sources, by leg

The two legs draw from **different** feeds — the anchor from real FX, the
basis from crypto venues.

| Leg                              | Role                                    |
| -------------------------------- | --------------------------------------- |
| FX anchor (`fiat/USD`)           | Fast, deep, exogenous cross             |
| Basis (`token/fiat`, `USDC/USD`) | Slow multiplicative correction near 1   |
| Static peg (`token/USD`)         | Last-resort constant, no feed behind it |

**Which venues serve each leg is not settled here.**
[`data-feeds.md`](data-feeds.md) §9 owns venue policy — which sources are
wired, on what terms, and why a named source was superseded — and this
section deliberately does not restate its roster, because the copy went
stale and began contradicting both that document and itself. What belongs
here is the model: what each leg *means*, and how the legs are resolved and
composed.

Two source properties the model does depend on, so they are stated rather
than left to be inferred:

- **A confidence half-width is what makes fm6 observable.** An anchor source
  that publishes one lets the model see *fresh-but-uncertain*; one that does
  not can only ever read as fresh or stale. This is why the anchor leg
  distinguishes sources at all.
- **Coverage is permanently asymmetric.** Only one of the seven demo markets
  reaches a CEX. For five of the rest an aggregator index *is* the basis leg,
  and the last has **no basis source at all** — its basis is pinned, so there
  is nothing for the resolution below to compare against. That is the standing
  condition the leg resolution is built around, not a temporary gap.

### Leg resolution

A leg is resolved from **every source that answered**, not from the first one
that did. The old ladder took the highest-priority live tier outright, which
made any single bad source the answer with nothing to contradict it — and
given the asymmetry above, most markets had exactly one source under them.

Per leg, per tick, across the healthy sources:

- **three or more** — the **median**, which one bad source cannot move;
- **two** — usable if they agree within the dispersion band. A disagreeing
  pair cannot adjudicate between itself, so the leg degrades instead of
  guessing — **unless exactly one of the two is a designated source**, which
  is the case the designation below exists to settle;
- **one** — an explicit single-source state. It still carries the mid, since
  refusing would dark most of the roster, but a lone source with **no
  designation** composes as `Unverified` rather than being described as a
  corroborated price. A lone *designated* source composes normally.

A **dispersion gate** rides alongside: when a leg's healthy sources span more
than the band, the leg is flagged, the source furthest from consensus is
named, and the composition **degrades**. Naming it is the point — a dispersion
alarm with no suspect attached is one nobody can act on. Degrading is the
other half: with three or more sources the median still resolves, so a
disagreement would otherwise be reported alongside a perfectly healthy mid.
Note the asymmetry with a lone uncorroborated source, which does *not*
degrade: that is a permanent condition, while a disagreement is a fault, and
only faults should tighten the kill switches.

The gate **generalizes** the one-shot startup wiring check, which survives
alongside it: that check could latch only once per market and spent its shot
on whichever source answered first, so an id reachable only through a
fallback went unvalidated until the day it was used.
What the one-shot still does that the gate cannot is attribute the very
*first* observation, where there is no history to have departed from.

A source may be **designated believable on its own**. Such a source anchors
its leg rather than averaging into it — blending a live first-party oracle
with a daily reference rate would only degrade the anchor the leg exists to
supply — and that designation is overridden when the source is itself the
outlier, so it cannot become a way for one bad feed to beat every check on
it.

Priority order survives only as the order sources are offered in, which
decides which ones fill a leg that has more than it can hold. It no longer
decides what the leg is worth.

#### Attribution

The bot surfaces, per leg per market, how many sources answered, which one
diverged, and — the contract this section states in full — **which sources the
value is actually composed of, and in what proportion**.

Attribution is a **set with weights**, not a single name. The ladder had a
well-defined "tier that answered" and consumers were built on it; under
consensus that concept does not survive, because the value usually belongs to
the set. Naming one contributor anyway would resurrect ladder semantics as a
lie dressed as data. The weights are **exact rather than heuristic**: every
resolution is a linear combination of contributor values, so each case has one
right answer.

| resolution                           | contributors                            |
| ------------------------------------ | --------------------------------------- |
| a designated source anchoring alone  | that source at `1.0`                    |
| a lone source, corroborated or not   | that source at `1.0`                    |
| median, odd count                    | the middle source at `1.0`              |
| median, even count                   | the two middle sources at `0.5` each    |
| an agreeing pair                     | both at `0.5` — the even case, with two |
| a dispersed pair with no designation | none; the leg resolves to nothing       |

Four properties bind any consumer of this:

- The weights **sum to 1 whenever the leg resolved to anything**, and the set
  is **empty whenever it did not**. An empty set means no value, never an
  unattributed one.
- A single name is offered **only for a singleton set**. An averaged pair has
  no dominant member, and picking a side of an exact tie is the ladder lie
  this replaces — so a single-name column renders null there rather than
  guessing.
- Contributors name the sources to **believe**; the dispersion outlier names
  the one to **distrust**. Reading either as the other is exactly backwards.
- A median's outer members are counted but **not credited**. They bound the
  answer without entering it, which is the robustness the median buys, so the
  source count and the contributor count legitimately differ.

Each contributor also carries **its own reading's age**, which is diagnostic
only. The leg's age remains the oldest age across *every healthy candidate* —
including the zero-weight outer members — so a leg can be older than every
contributor credited. Excluding the uncredited members would let a stale
source vanish from the one number that polices staleness, so every freshness
and staleness test keeps reading the leg's age, never a contributor's.

Source names here are the **bare venue** vocabulary. That is not always the
feed adapter's own source name: a venue whose endpoint is per *product* names
itself per product (`coinbase:EURC-USDC`) while the tag stays `coinbase`.
Widening the tag is deliberately not the fix — a per-product name is built at
runtime, which would cost an allocation per contributor per tick on the
quoting hot path — so a consumer joining to per-feed health matches on the `:`
prefix rather than on equality. A mismatched join here fails **silently**,
returning nothing rather than erroring, which is why the rule is stated rather
than left to be inferred.

### Basis estimation

`basis` is a **slow, smoothed multiplicative correction**, not a chased
price: an EMA over the live basis observations. A Kalman filter is
warranted only if the bot fuses several basis sources or drives spread
width from the basis variance — deferred (§5). The smoothing half-life is
**TBD — set by the basis-process characterization** over collected
history (`data-feeds.md` §11); it is not guessed here.

Two properties keep that smoothing from being defeated by a single reading,
both of which matter because the estimate is *multiplied into every quote*:

- **No one observation may replace the estimate.** The blend weight rises
  with the gap since the last update, so a returning observation re-seeds
  rather than crawling off a stale estimate — but uncapped, "re-seed" means
  one print *becomes* the basis at every gap boundary, which is to say every
  session reopen and every outage recovery. The weight is capped below 1, and
  an observation too far from the running estimate is refused rather than
  smoothed: the basis is a slow process by construction, so a large
  single-tick move is a bad source, not news. A refusal is reported, never
  silent. Note the scope of that guard precisely — it bounds the size of any
  **single** step, and it measures against the running estimate, so it does
  not bound cumulative drift across many accepted steps. The sane band is
  what stops a slow walk, which makes that band load-bearing rather than
  merely a peg-event alarm.
- **A carried basis expires.** Every input leg is bounded by an age; the
  estimate itself must be too, or a basis smoothed seconds ago and one
  smoothed days ago produce identical quotes. Past the bound the model stops
  quoting on the dead estimate and falls to the static peg — or pauses, when
  the market has no static peg to fall to. It never substitutes 1.0 for an
  unobserved basis — a fabricated parity claim is indistinguishable in the
  output from having measured the basis and found it at par.

The refusal rule and the expiry interact deliberately: a source stuck on a
bad value has its prints refused, the estimate stops being refreshed, and the
age bound eventually retires it. A persistent disagreement therefore degrades
the market rather than walking the basis to a wrong level.

### Composition

For one market, per tick:

```text
fx    = live FX anchor (fiat/USD)
basis = EMA of (token/fiat ÷ USDC/USD) over its window
fair  = fx × basis        # the mid the rest of this spec refers to
```

`fair` replaces the old "first live tier is the mid" cascade: no single
tier *is* the price — two legs compose one. A missing or stale leg is a
regime change (below), not a silent failover to a lower-quality mid.

### Regimes and failure modes

The model is only as sound as its legs, and each way a leg can fail is a
first-class regime, not an exception:

1. **USDC common-mode.** A USDC depeg moves the `USDC/USD` leg of
   **every** market's basis at once — a correlated, portfolio-wide event
   the per-market FX anchors say nothing about. It needs a **separate
   USDC/USD anchor** and a portfolio-level guard, not seven independent
   per-market checks.
1. **Weekend / session role-flip.** Interbank FX is closed Fri ~5pm →
   Sun ~5pm ET — structural, not an outage. On weekends the crypto
   reference is the **only** live price discovery, so the model
   **switches the anchor to the crypto reference** for that window rather
   than treating FX-stale as "fall back to a static peg." The Sunday
   reopen is the reversion / gap event — a taker's moment, and a maker's
   risk to brace into. Which crypto venues are live in that window is
   [`data-feeds.md`](data-feeds.md) §9's to say; naming them here is what
   left this line asserting a venue the same document had ruled out as
   unreachable.
1. **Per-market reversion is a gate, not a global truth.** The basis
   mean-reverts only as hard as redemption arbitrage enforces it: strong
   for EURC (Circle Mint), weak or absent for the thin exotics (VCHF,
   TGBP, ZARP, MXNe, XSGD, IDRX). "Basis reverts" is asserted per market,
   never assumed for the roster.
1. **Redemption arb suspends under stress.** Circle paused USDC
   redemptions over the SVB weekend; a "temporary" dislocation can
   persist for days. Never size a position as if reversion is guaranteed
   on any horizon.
1. **Reflexivity of the crypto/USD fallback.** A thin token's
   CoinGecko / CMC price echoes its one venue — using it as the anchor
   feeds our own prints back to us. This is why it is a fallback of last
   resort, never the driver.
1. **Confidence widens at the edge moment.** Around ECB / FOMC / NFP the
   FX oracle's confidence interval blows out precisely when the move
   happens. Separate **fresh-but-uncertain** (quote, but widen the
   spread) from **stale** (do not quote) — a wide confidence band is not
   a dead feed.
1. **Ladder vs macro vol.** The §2 ladder assumes a calm σ; the regimes
   that create the edge are macro spikes that sweep a static ladder. This
   promotes the deferred realized-σ estimator (§3 cold-path trigger 2)
   from a nicety toward load-bearing.

### Degraded and halt conditions

The composition maps onto the kill-switch policy (§4):

- **Basis-band breach** — `basis` outside its per-market sane band → halt
  quotes (peg event). The band is **TBD — set per market by the
  basis-process characterization** (`data-feeds.md` §11); the old fixed
  `[0.97, 1.03]` and its "300 bps for a Monday gap" rationale were guesses
  and are **not** reasserted here.
- **FX anchor stale (outside the weekend regime)** — no live anchor when
  one is expected → run degraded (§4). Inside the weekend regime this is
  the normal state, not a fault: the crypto reference is the anchor.
- **USDC/USD anchor breach** — the portfolio-wide guard of failure
  mode 1.

### Polling cadence

| Leg                          | Cadence                                                                 |
| ---------------------------- | ----------------------------------------------------------------------- |
| FX anchor                    | Streamed (Pyth Hermes / OANDA push); no fixed poll                      |
| Basis (crypto venues)        | Slow poll — the basis is smoothed, so sub-second freshness buys nothing |
| Peg-truth / daily references | Slowest — issuer rate and ECB publish on the order of a day             |

Exact intervals are **TBD — set by the per-venue budget**
(`data-feeds.md` §10); every staleness / session threshold is **TBD —
set by the flow-regime, lead-lag, and observability analyses**
(`data-feeds.md` §11). `fair` is recomputed every tick;
`SetReferencePrice` fires only per the §3 cadence rules, not on every
observation.

______________________________________________________________________

## 2. Profile math

Relevant protocol facts (see **architecture.md → LiquidityProfile** and
**→ Flush**):

- `N_LEVELS = 8` bids + 8 asks per vault.

- Each `Level` is:

  ```text
  { price_offset: Ppm32, size_bps: u16,
    expiry_offset_secs: u32, expiry_offset_slots: u32 }
  ```

- `price_offset` is ppm from `reference_price.price` — bids subtract,
  asks add.

- `size_bps` is fraction of the **inventory leg**: `quote_atoms` for
  bids (USDC), `base_atoms` for asks (the token).

- **Invariant:** `Σ size_bps ≤ 10000` per side.

- Sizes auto-rescale to current inventory on each flush; the leader
  doesn't manage absolute atoms.

### Proposed ladder

Per side, symmetric at launch (~\$100 per leg, full leg committed):

| Level | `price_offset` | bps from mid | `size_bps` | depth at launch | cumulative |
| ----- | -------------- | ------------ | ---------- | --------------- | ---------- |
| 1     | 5_000 ppm      | 50 bps       | 4000 (40%) | ~\$40           | \$40       |
| 2     | 10_000 ppm     | 100 bps      | 3000 (30%) | ~\$30           | \$70       |
| 3     | 20_000 ppm     | 200 bps      | 2000 (20%) | ~\$20           | \$90       |
| 4     | 50_000 ppm     | 500 bps      | 1000 (10%) | ~\$10           | \$100      |
| 5-8   | 0 / unused     | —            | 0          | —               | —          |

(Unit conversion: `1_000_000 ppm = 100% = 10_000 bps`, so **1 bp = 100 ppm**.)

The seed profile the bootstrap stamps is simpler — the whole leg at the
single 50 bps level, so the opening book flashes ~\$100 at top of book —
and the maker bot re-arms the laddered shape above on its first tick.

Properties:

- `Σ size_bps = 10000` per side — fully commits the leg, no reserve.
- Top-to-top spread = `2 × 50 bps = 100 bps`.
- Effective spread widens by level beyond the top: cumulative VWAP
  half-spread to clear the whole \$100 leg is
  `(40·50 + 30·100 + 20·200 + 10·500)/100 = 140 bps`.

### Justification (Avellaneda–Stoikov)

The shape is hand-tuned but anchored to A-S
([Avellaneda & Stoikov 2008, *High-frequency trading in a limit order
book*][as2008]), equation (3.18) for the half-spread:

```text
half_spread = γ·σ²·τ/2 + (1/γ)·ln(1 + γ/κ)
```

For a stable-pair scale (realized daily vol ≈ 50 bps →
σ ≈ 5e-3 / √86_400 ≈ 1.7e-5 in price-units-per-√sec), small τ, and
γ = 0.1: the inventory term `γσ²τ/2` is negligible; the half-spread is
dominated by the `(1/γ)·ln(1+γ/κ)` fill-intensity term, which with the
dropset-alpha defaults (κ from `FILL_DECAY_STEPS = 10`,
`PRICE_STEP = 0.0001`) comes out around **50 bps** — Level 1.

Geometric widening (50 → 100 → 200 → 500 bps) approximates the A-S
quote-intensity curve `λ(δ) = A·exp(-κδ)`: each doubling of `δ` cuts the
fill rate by ~`exp(-κΔ)`, so doubling size at deeper levels keeps the
expected fill rate per level roughly flat. A crisper derivation of
optimal per-level offsets and sizes for finite inventory `Q` lives in
[Guéant, Lehalle & Fernandez-Tapia 2011, *Dealing with the inventory
risk*][gueant2011] and [Guéant 2017, *Optimal market
making*][gueant2017] — deferred to a follow-up. The hand ladder is the
shipped shape until the volatility-driven ladder replaces it.

dropset-alpha already implements the A-S formulae in
[`calculate_spreads.rs`][alpha-spreads] and
[`parameters.rs`][alpha-params] — the math is portable, the
venue-specific order placement is not.

### Inventory skew (A-S reservation price, with override)

When fills push the vault off neutral, shift the **reference price**
rather than reshape the profile. A-S equation (3.17):

```text
r = mid - q · γ · σ² · τ
```

In our terms, with `q` = signed inventory deviation in USDC-equivalent
atoms:

```text
q          = (base_value_in_USDC - quote_atoms_USDC) / 2
δ_ref_bps  = -q · γ · σ² · τ / mid · 10000
```

The factor of 2 expresses **deviation from neutral**: a $10 swing
between legs means each side has moved $5 off the midpoint, so the
signed deviation is $5, not $10.

For these stable-pair vaults the formal A-S skew comes out sub-bps —
too small to matter.

**Override with a linear inventory skew** instead: shift reference by
**0.5 bps per 1% of TVL of deviation**, capped at ±20 bps. This is a
hand-tuned override of A-S because the stable-pair σ is so small that the
formal A-S skew is invisible at our size. The rate is keyed to
*fractional* deviation, not absolute dollars, so one calibration holds at
any vault size — the multi-market demo seeds ~\$100 top-of-book across
markets whose tokens span ~\$1.14 down to ~\$0.00006, and the skew must
mean the same thing in each. At the \$100 reference vault this reproduces
the original "5 bps per \$10" (a \$10 deviation is 10% of a \$100 TVL).
Beyond a 15%-of-TVL deviation (a 30% per-side imbalance), reshape the
ladder via `SetLiquidityProfile` (see §3).

______________________________________________________________________

## 3. Update cadence

### `SetReferencePrice` triggers (hot path)

`SetReferencePrice` is two aligned `u64` stores — the cheap path. Call
when **any** of:

1. `|mid - last_set_price| / last_set_price > 10 bps` (price drift).
1. Heartbeat: 30 s elapsed since last set.
1. Inventory skew rule fires: `δ_ref_bps` changes by > 2 bps.

Expected: **2–6 calls per minute** in calm conditions. The
`quote_slot` argument can be pre-signed at an older slot if relay
latency matters (see **architecture.md → SetReferencePrice**;
`MAX_BACKDATE = 50 slots ≈ 20 s`).

### `SetLiquidityProfile` triggers (cold path)

`SetLiquidityProfile` rewrites the full ladder and arms a flush on the
next take. Call when **any** of:

1. Per-side inventory imbalance > 30% from launch.
1. Realized σ over a 24 h window has doubled (vol-regime change).
1. Daily heartbeat (once per UTC day, fixed time).

Expected: **1–3 calls per day** per market.

### Per-level expiry — two offsets

Expiry is **dual-domain** (architecture.md → **Expiry — the dual gate**):
each level carries a wall offset and a slot offset, and rests only while
it is inside both. The wall column is the policy that used to be
expressed in slots at the ~0.4 s/slot pace mainnet then ran at; the
nominal lives are unchanged, but they now hold **through a halt**, where
slots stop ticking and wall time does not.

| Level | secs  | wall-clock | slots    |
| ----- | ----- | ---------- | -------- |
| 1     | 36    | ~36 s      | 2        |
| 2     | 120   | ~2 min     | 30       |
| 3     | 480   | ~8 min     | 300      |
| 4     | 2_880 | ~48 min    | no bound |

Top-of-book expires fast so a dead bot doesn't bleed against stale
prices; deep levels live longer because they rarely fill and we don't
want to churn `SetReferencePrice` just to keep them alive. Per-level
expiry stratification is an explicit feature of the protocol (see
**architecture.md → LiquidityProfile → Flush**).

**Why the slot column exists.** The cluster clock is second-denominated
and accurate to only a few seconds, which floors any wall TIF at ~15 s —
a ~37-slot dead-man tail behind a prop-cadence quoter. The slot bound is
where top-of-book gets a *sub-second* deadline instead: 2 slots means
the level dies almost immediately unless the next tick re-stamps it. It
widens with depth, and the deepest tier is left slot-unbounded (the max
offset, not a sentinel) so its stratified wall decay governs it. These
are the shape of the policy, not a calibration — the vol-ladder retune
owns the tuning.

Expiry still isn't the whole answer to a dead bot: the wall bound now
caps a resting level's life through a halt, but 48 minutes of unattended
drift on the deepest tier is far longer than the bot means to rest a
book it is no longer refreshing. It kills its own book rather than
waiting for expiry — see **§4 → Stale-quote invalidation**.

**Invariant:** Level 1 wall expiry must exceed the `SetReferencePrice`
heartbeat (30 s here; 36 s gives ~6 s safety margin),
otherwise top-of-book goes dark in the gap between expiry and the
next forced refresh. `quote_slot` backdating (up to
`MAX_BACKDATE = 50 slots ≈ 20 s`) shifts every level's absolute
expiry back by the same amount, which would wipe out the L1 margin
entirely. **Rule:** do not backdate `SetReferencePrice` on the
heartbeat path. Backdating is only safe for cold-path
`SetLiquidityProfile` reshapes, where the L2+ expiries (≥ 2 min)
absorb the shift trivially.

### Bot heartbeat

**5-second tick.** One supervisor refreshes the batched feeds once, then
walks each market: recompute `mid` → evaluate triggers → fire at most one
ix for that market. No retry storms: if an ix fails, skip the market this
tick and retry on the next one.

### Fill detection

Hot-path ix emit nothing — see **architecture.md → Events and emission**.
The bot detects fills by subscribing to the `take` ix events emitted via
`emit_cpi!` (full fidelity, never dropped). One subscription covers every
market the leader quotes; the supervisor routes each fill to its market by
`event.market`. A per-tick vault-read state diff is the fallback. **Do
not poll account state for fills** — it is too slow.

______________________________________________________________________

## 4. Inventory bounds & kill switches

| Trigger                                      | Action                                                                         |
| -------------------------------------------- | ------------------------------------------------------------------------------ |
| Imbalance > 30% from launch                  | Reshape: shrink the accumulating side so the heavy side dominates and offloads |
| Imbalance > 50%                              | Freeze heavy side (zero `size_bps` on that side; only the rebuild side quotes) |
| Imbalance > 80%                              | `FreezeVault` — alert and review by hand                                       |
| `basis` outside its per-market band (§1)     | `FreezeVault` (peg event) — band is TBD by analytics (`data-feeds.md` §11)     |
| USDC/USD anchor breach (common-mode, §1)     | `FreezeVault` portfolio-wide — one depeg hits every market's basis at once     |
| FX anchor stale outside the weekend regime   | Run degraded; tighten kill switches by 50%                                     |
| Basis (crypto) leg also down → last fallback | Full degrade (the deepest degraded case)                                       |
| Vault TVL drops below 80% of launch TVL      | `FreezeVault`, post-mortem                                                     |

`FreezeVault` is admin-only and irreversible, so the bot maps these hard
triggers to a leader-authorized halt (zero the profile, kill the resting
book, alert) rather than calling `FreezeVault` autonomously; a real
freeze stays a human decision. Per-market TVL-floor and skew calibration
is coordinated separately.

### Stale-quote invalidation

Zeroing the profile stops the *next* flush from materializing levels; it
does not touch the levels already resting, which stay matchable until a
deadline passes — up to ~48 min on the deepest tier. Expiry now *does*
bound that in wall-clock terms (the wall conjunct is measured from the
quote's `quote_unix` datum, so a halt no longer freezes the countdown the
way slot-only expiry did), but a cap is not a policy: 48 minutes is far
longer than the bot means to leave a book unattended. Any gap in which
nobody refreshes the reference — a bot
restart, a chain halt, feeds gone dark — is a window in which takers can
fill against a price the bot no longer stands behind.

The bot closes that window without needing `FreezeVault`. Matching skips
any vault failing `has_valid_reference_price()` (**architecture.md →
Order matching → Book construction**), and the zero sentinel fails it, so
one `SetReferencePrice` at `price = 0` — the ordinary quote-authority hot
path — takes the whole vault's book dark while leaving the
`LiquidityProfile` intact. The next live reference re-arms the same shape;
nothing has to be rebuilt.

| Situation                                         | Kill the resting book                              |
| ------------------------------------------------- | -------------------------------------------------- |
| Startup, last live quote older than the bound     | Yes, **before** the first quote of the run         |
| Startup, last live quote's age unknown            | Yes — no freshness evidence reads as stale         |
| Startup, last live quote inside the bound         | No; go straight to quoting                         |
| Running, no usable feed for longer than the bound | Yes, instead of holding the reference indefinitely |
| Running, kill-switch halt                         | Yes, **first** — ahead of zeroing the profile      |

On a halt the kill stamp goes **first**, ahead of zeroing the profile.
Standing down takes two instructions and the bot's cycle budget is one,
and only the stamp stops a taker: zeroing the profile prevents the next
flush from materializing more levels but does nothing about the ones
already resting. Ordering it this way also keeps a persistently failing
profile send from starving the stamp, since a halt is exactly the moment
a stale resting price is most worth picking off.

The staleness bound is **60 s**: twice the `SetReferencePrice` heartbeat,
so a healthy bot's own last stamp is always well inside it and an
ordinary restart doesn't churn the book, yet only ~2% of the deepest
tier's wall-clock life, so no level rests unattended for more than about
a minute. The kill stamp carries a **priority fee** because it races
takers in the first blocks after the bot returns; losing that race by one
block is the residual exposure this accepts.

Age is measured against the bot's **own persisted wall-clock record** of
its last live stamp, one file per market. The vault stores `quote_slot`,
not a timestamp, and slot arithmetic is exactly what a halt invalidates —
so the timestamp cannot be derived from a chain read. A missing,
unreadable, or future-dated record reads as unknown, which counts as
stale: every failure mode lands on the safe side.

This is the unconditional half of the halt / pick-off mitigation — it
holds regardless of the runtime's `Clock` behavior, so it stays required
alongside the program-side dual expiry gate. That matters more than it
sounds: which conjunct binds after a restart depends on the cluster's
clock design (architecture.md → **Expiry — the dual gate**), and this
mitigation depends on neither.

______________________________________________________________________

## 5. Explicitly deferred

- Full A-S optimization for finite `Q` ([Guéant 2017][gueant2017]) — the
  shipped ladder is hand-tuned (§2).
- Weighted multi-oracle fusion — a Kalman filter blending several basis
  sources (and driving spread width from the basis variance), or fusing
  many simultaneous venues (Jupiter, Raydium, Orca, Manifest, DFlow, …).
  The bot smooths **one** source per leg with an EMA (§1) and fails over
  rather than blending.
- Adversarial taker bot for hardening — separate effort. A *benign*
  stochastic flow taker does ship (a quiet/burst Markov arrival process
  with LogNormal order sizes) to move the book and exercise the maker;
  the adversarial strategy-hardening taker remains deferred.
- Hedging / shorting for market-neutrality — separate effort, requires
  venue research.
- Performance fee / outside-depositor flow — comes with vault maturity.
- Delegated per-market `quote_authority` hot keys and the devnet/mainnet
  promotion — tracked separately under the hardening umbrella (see
  **Status**); this spec stops at the strategy the leader runs, whatever
  network it runs against.

______________________________________________________________________

## 6. Operational telemetry

The read side of running the bot: what it is quoting, what each tick
decided, and whether its inputs are alive. The bot writes; Grafana
renders and alerts. Nothing reads these tables back into a decision, so
this whole section is a tap on state the quote loop already computed
(`bots/maker-bot/src/telemetry.rs`).

Numbered §6 rather than slotted before the deferred list because §5 is
cited as "deferred" from elsewhere in the tree, and renumbering it would
silently redirect those references.

### Three tables

- **`maker_telemetry`** — one sample per market per tick: the composed
  fair value, the three references that differ (this tick's candidate,
  what this process last stamped, and what the vault actually carries),
  the implied touch, the valued inventory, the composition regime, and
  the kill-switch decision.
- **`maker_legs`** — one row per market per leg per tick: the leg's
  resolved value, the age **the engine aged it by**, its confidence
  half-width where the resolved reading carries one, and the three
  consensus diagnostics below.
- **`feed_health`** — current liveness per registered feed source,
  upserted in place.

DDL lives in `db-schema/migrations/0003_maker_telemetry.sql`, which
carries the per-column reasoning; the single-schema-owner rule (see
`docs/data-feeds.md` §8) means the bot issues no DDL and never asserts a
schema.

### The tick outcome is recorded on every path

A tick can end at six points — the vault read failing, a frozen vault,
a paused composition, a halt, a freeze-side, an ordinary quote — and the
sample is emitted on all of them, including the error path. This is the
non-obvious requirement: emitting only from the happy path yields a
dashboard that goes *blank* precisely when something is wrong, which is
indistinguishable from the bot having died. So `action` carries four
values that are not `Action` variants — `Pause`, `Frozen`, `TickError`,
and `Unknown` — for the states the policy never got to decide.

`TickError` and a decision are not exclusive, which matters to anyone
writing a query over this column: a tick that decided `Halt` and then
failed to send the instruction records `action = 'Halt'` with a non-NULL
`tick_error`, because the decision is the more alarming fact and the
kill-switch alert keys on it. So count tick failures by
`tick_error IS NOT NULL`, never by `action = 'TickError'`. `Unknown` is
the residue — no decision *and* no failure — which no path currently
produces; read it as a defect signal rather than a quiet tick.

Correspondingly, a column is `NOT NULL` only if *every* one of those
paths can fill it honestly. A NULL means "this tick could not know",
which is not zero — an unknown skew and a zero skew are different
facts, as are an unread vault and an empty one. The dashboards leave
gaps rather than plotting zero.

### Per-feed health is generic; per-leg rows carry consensus, not attribution

Feed liveness rides the feeds runner's existing `FeedMetrics` seam
(`docs/data-feeds.md` §13), so a source that is merely *registered*
gets a row: a venue adapter added later appears with no per-feed wiring
and no dashboard change.

What that seam carries bounds what it can say, and the bound shapes the
schema. The runner hands a recorder a feed *name* and batch stats,
never the records — and the maker's price sources are **venue**-level
(`pyth-hermes`, `kraken`), each yielding a map of many instruments per
batch. So a `last_value` column on a per-source row would have to pick
one instrument arbitrarily. Liveness therefore lives in `feed_health`,
and readings live in `maker_legs`.

**There is deliberately no "which feed supplied this leg" column**, and
that follows from the resolver rather than being an omission. A leg is
a *candidate set* resolved by consensus (§1): several sources
contribute and the value is a summary of them — a median, or a
designated source that survived contradiction. There is no single
answering venue to name, and naming one anyway would mean picking
arbitrarily while presenting the pick as authoritative.

What the rows carry instead is what is actually knowable:

- **`consensus_state`** — how well corroborated the leg was. Six
  values, and every reader must enumerate all six: `Absent`,
  `Corroborated` (3+ inside the band), `Agreed` (exactly two),
  `SingleTrusted`, `SingleUnverified`, `Dispersed`. `Absent` is
  enumerated for completeness but is not written here: a leg that
  resolved to nothing contributes **no row**, so its absence shows up as
  a gap in the series. Look for the missing row, not for the value.
- **`contributor_count`** — how many healthy sources resolved it.
- **`dispersion_outlier`** — when dispersed, the source *furthest from*
  the consensus. This is the **suspect**, the least representative
  member of the set — emphatically not "the feed that answered", which
  would be exactly backwards.

`SingleTrusted` and `SingleUnverified` must never be collapsed.
`SingleUnverified` is the **steady state** for a market with no second
source — most of this roster — rather than a fault, and it is the only
signal that a market is being quoted off one unchecked feed. Merging
the two would erase precisely that, and worst on the thin markets where
it matters most. (The per-currency source-floor survey predicts which
markets sit there permanently, so a market appearing there
*unpredicted* is a real signal.)

Per-source attribution returns later as an **additive** migration, once
the resolver exposes a contributor set with weights; that shape is
already decided, and this table does not approximate it early. The
`pub const FEED_NAME` values in each `feeds::venues` module remain the
health table's keys, and stay constants because that key is a
cross-crate contract — a renamed source would otherwise empty a panel
silently, with no build error. Note one asymmetry when joining
`dispersion_outlier` to them: the resolver offers the bare venue
(`coinbase`) while the spot source is named per product
(`coinbase:EURC-USDC`), so that join is a prefix match on the `:`,
not equality.

### Fire-and-forget, and what that costs

The quote loop is synchronous and Postgres is not, so a sample is
`try_send` onto a bounded channel and the tick moves on; a background
task drains it. Three consequences, all deliberate:

- **A full channel drops the sample.** A maker that stalls its quote
  loop behind a slow write is worse than a gap in a chart.
- **A database outage does not stop telemetry permanently.** The sink
  is wrapped best-effort, so a failed batch is dropped and logged
  rather than killing the runner — which would otherwise leave the bot
  blind for the rest of its life after one blip. The pool is also lazy,
  so losing a startup race against Postgres costs a few samples rather
  than the whole run.
- **Delivery is therefore at-most-once.** Sound only because every
  record here is a sample of current state that the next tick
  supersedes. None of this may be reused for the fill/event path, where
  the records *are* the product.

### Dashboards and alerts

`market-data/grafana/dashboards/maker-operations.json`, provisioned
from the repo alongside the market-data dashboards, with alert rules in
`market-data/grafana/provisioning/alerting/maker.yml`: dead heartbeat,
stale feed, and degraded-or-halted. The rules evaluate and reach Firing
in Grafana's UI; they deliver nowhere, because a real destination needs
a secret and secrets are not committed.

One ambiguity is inherent rather than an oversight: because telemetry is
fire-and-forget, a dead heartbeat means *either* the maker stopped *or*
the maker is healthy and cannot reach Postgres. Separating them needs a
signal that does not travel over the database. The feed-health table
narrows it in practice — a live bot with a dead database shows every
feed stale at the same instant.

[alpha-params]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/parameters.rs#L11
[alpha-spreads]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/calculate_spreads.rs#L41
[as2008]: https://people.orie.cornell.edu/sfs33/LimitOrderBook.pdf
[gueant2011]: https://arxiv.org/abs/1105.3115
[gueant2017]: https://arxiv.org/abs/1605.01862
