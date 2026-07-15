<!-- cspell:word fomc -->

<!-- cspell:word guéant -->

<!-- cspell:word illiquidity -->

<!-- cspell:word kalman -->

<!-- cspell:word laggy -->

<!-- cspell:word lehalle -->

<!-- cspell:word oanda -->

<!-- cspell:word parameterizes -->

<!-- cspell:word raydium -->

<!-- cspell:word stoikov -->

<!-- cspell:word tapia -->

# Market Making MVP — multi-market FX stablecoins

The **operating spec** for Dropset's market-making vaults: how a single
leader bot quotes a roster of non-USD FX stablecoins against USDC on the
eCLOB, each at a 100 bps spread with \$100 of top-of-book inventory per
side. It pins down the four numbers a bot needs and nothing more —
reference-price construction, the `LiquidityProfile` ladder, update
cadence, and the inventory/kill-switch policy. The math is anchored to
Avellaneda–Stoikov but the ladder is hand-shaped: stable-pair σ is too
small for the formal A-S skew to matter at this size, so the bot uses A-S
as a sanity check and a linear override for inventory.

**Doc boundary.** Dependency flows one way: **`market-making-mvp.md` →
`architecture.md`**. This document references *down* into the protocol
spec (`LiquidityProfile`, `SetReferencePrice`, `SetLiquidityProfile`,
`FreezeVault`, flush math) and never the other way around. Nothing in
the protocol depends on this strategy — a different leader can run a
different shape against the same instructions.

**Objective.** Breadth, not yield. The demo flashes credible top-of-book
across many FX stablecoins at once, all routed through one SDK — the
illiquidity story is that these markets have no liquid home on Solana,
and Dropset's maker stack can stand one up on demand. The MVP exercises
the leader interfaces end to end (price/profile cadence, inventory drift,
peg-deviation alarms) across the whole roster.

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
- Single leader across all markets on localnet. No hedging, no shorting,
  no leverage. The delegated per-market `quote_authority` model is the
  devnet/mainnet promotion's concern.

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
derived in part from the very Orca / CEX prints we race), so anchoring on
it makes the bot lag exactly when the edge appears.

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
basis from crypto venues. The token's crypto/USD price (CoinGecko / CMC)
is **demoted to a last-resort fallback** for the reasons above.

| Leg                              | Primary                                     | Peg-truth / cross-check         | Fallback                          |
| -------------------------------- | ------------------------------------------- | ------------------------------- | --------------------------------- |
| FX anchor (`fiat/USD`)           | Pyth Hermes FX / OANDA, streaming           | CME 6E during session hours     | ECB / Frankfurter daily reference |
| Basis (`token/fiat`, `USDC/USD`) | Coinbase `<token>/USDC`, Binance `EUR/USDT` | Circle / issuer redemption rate | CoinGecko / CMC token/USD         |

The bot surfaces which source is live per leg, per market.

### Basis estimation

`basis` is a **slow, smoothed multiplicative correction**, not a chased
price: an EMA over the live basis observations. A Kalman filter is
warranted only if the bot fuses several basis sources or drives spread
width from the basis variance — deferred (§5). The smoothing half-life is
**TBD — set by the survey's basis-process characterization**
(`fx-survey.md` §6); it is not guessed here.

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
1. **Weekend / session role-flip.** Interbank FX and CME 6E are closed
   Fri ~5pm → Sun ~5pm ET — structural, not an outage. On weekends the
   crypto reference (Coinbase `<token>/USDC`, Binance `EUR/USDT`) is the
   **only** live EUR/USD price discovery, so the model **switches the
   anchor to the crypto reference** for that window rather than treating
   FX-stale as "fall back to a static peg." The CME Sunday reopen is the
   reversion / gap event — a taker's moment, and a maker's risk to brace
   into.
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
  quotes (peg event). The band is **TBD — set per market by the survey's
  basis-process characterization** (`fx-survey.md` §6); the old fixed
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

Exact intervals, and every staleness / session threshold, are **TBD —
set by the survey's flow-regime and observability analyses**
(`fx-survey.md` §6). `fair` is recomputed every tick;
`SetReferencePrice` fires only per the §3 cadence rules, not on every
observation.

______________________________________________________________________

## 2. Profile math

Relevant protocol facts (see **architecture.md → LiquidityProfile** and
**→ Flush**):

- `N_LEVELS = 8` bids + 8 asks per vault.
- Each `Level = { price_offset: Ppm32, size_bps: u16, expiry_offset: u32 }`.
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
making*][gueant2017] — deferred to a follow-up. For the MVP the hand
ladder is good enough.

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

### Per-level `expiry_offset` (slots ≈ 0.4 s each on mainnet)

| Level | offset (slots) | wall-clock |
| ----- | -------------- | ---------- |
| 1     | 90             | ~36 s      |
| 2     | 300            | ~2 min     |
| 3     | 1_200          | ~8 min     |
| 4     | 7_200          | ~50 min    |

Top-of-book expires fast so a dead bot doesn't bleed against stale
prices; deep levels live longer because they rarely fill and we don't
want to churn `SetReferencePrice` just to keep them alive. Per-level
expiry stratification is an explicit feature of the protocol (see
**architecture.md → LiquidityProfile → Flush**).

**Invariant:** Level 1 expiry must exceed the `SetReferencePrice`
heartbeat (30 s here, 90 slots ≈ 36 s gives ~6 s safety margin),
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
| `basis` outside its per-market band (§1)     | `FreezeVault` (peg event) — band is TBD by survey (`fx-survey.md` §6)          |
| USDC/USD anchor breach (common-mode, §1)     | `FreezeVault` portfolio-wide — one depeg hits every market's basis at once     |
| FX anchor stale outside the weekend regime   | Run degraded; tighten kill switches by 50%                                     |
| Basis (crypto) leg also down → last fallback | Full degrade (the deepest degraded case)                                       |
| Vault TVL drops below 80% of launch TVL      | `FreezeVault`, post-mortem                                                     |

`FreezeVault` is admin-only and irreversible, so the bot maps these hard
triggers to a leader-authorized halt (zero the profile, let levels
expire, alert) rather than calling `FreezeVault` autonomously; a real
freeze stays a human decision. Per-market TVL-floor and skew calibration
is coordinated separately.

______________________________________________________________________

## 5. Explicitly deferred

- Full A-S optimization for finite `Q` ([Guéant 2017][gueant2017]) —
  hand-tuned ladder for MVP.
- Weighted multi-oracle fusion — a Kalman filter blending several basis
  sources (and driving spread width from the basis variance), or fusing
  many simultaneous venues (Jupiter, Raydium, Orca, Manifest, DFlow, …).
  The MVP smooths **one** source per leg with an EMA (§1) and fails over
  rather than blending.
- Adversarial taker bot for hardening — separate effort. The localnet MVP
  does ship a *benign* stochastic flow taker (a quiet/burst Markov arrival
  process with LogNormal order sizes) to move the book and exercise the
  maker; the adversarial strategy-hardening taker remains deferred.
- Hedging / shorting for market-neutrality — separate effort, requires
  venue research.
- Performance fee / outside-depositor flow — comes with vault maturity,
  not MVP.
- Delegated per-market `quote_authority` hot keys and the
  devnet/mainnet promotion — tracked separately; this spec is the
  localnet plumbing.

[alpha-params]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/parameters.rs#L11
[alpha-spreads]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/calculate_spreads.rs#L41
[as2008]: https://people.orie.cornell.edu/sfs33/LimitOrderBook.pdf
[gueant2011]: https://arxiv.org/abs/1105.3115
[gueant2017]: https://arxiv.org/abs/1605.01862
