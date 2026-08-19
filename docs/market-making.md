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
- **Coverage is permanently asymmetric.** Only EURC of the seven demo markets
  reaches a CEX, so for the other six an aggregator index *is* the basis leg.
  That is the standing condition the leg resolution below is built around, not
  a temporary gap.

### Leg resolution

A leg is resolved from **every source that answered**, not from the first one
that did. The old ladder took the highest-priority live tier outright, which
made any single bad source the answer with nothing to contradict it — and
given the asymmetry above, most markets had exactly one source under them.

Per leg, per tick, across the healthy sources:

- **three or more** — the **median**, which one bad source cannot move;
- **two** — usable if they agree within the dispersion band; a disagreeing
  pair cannot adjudicate between itself, so the leg degrades instead of
  guessing;
- **one** — an explicit single-source state. It still carries the mid, since
  refusing would dark most of the roster, but the composition reports
  `Unverified` rather than describing an unchecked feed as a corroborated
  price.

A **dispersion gate** rides alongside: when a leg's healthy sources span more
than the band, the leg is flagged and the source furthest from consensus is
named. Naming it is the point — a dispersion alarm with no suspect attached
is one nobody can act on. The gate is the general form of the one-shot
startup wiring check it replaces: that check could latch only once per
market and spent its shot on whichever source answered first, so an id
reachable only through a fallback went unvalidated until the day it was used.

A source may be **designated believable on its own**. Such a source anchors
its leg rather than averaging into it — blending a live first-party oracle
with a daily reference rate would only degrade the anchor the leg exists to
supply — and that designation is overridden when the source is itself the
outlier, so it cannot become a way for one bad feed to beat every check on
it.

Priority order survives only as the order sources are offered in, which
decides which ones fill a leg that has more than it can hold. It no longer
decides what the leg is worth.

The bot surfaces, per leg per market, how many sources answered and which one
diverged.

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
  silent.
- **A carried basis expires.** Every input leg is bounded by an age; the
  estimate itself must be too, or a basis smoothed seconds ago and one
  smoothed days ago produce identical quotes. Past the bound the model stops
  quoting on the dead estimate and falls to the static peg. It never
  substitutes 1.0 for an unobserved basis — a fabricated parity claim is
  indistinguishable in the output from having measured the basis and found it
  at par.

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

[alpha-params]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/parameters.rs#L11
[alpha-spreads]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/calculate_spreads.rs#L41
[as2008]: https://people.orie.cornell.edu/sfs33/LimitOrderBook.pdf
[gueant2011]: https://arxiv.org/abs/1105.3115
[gueant2017]: https://arxiv.org/abs/1605.01862
