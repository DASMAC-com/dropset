<!-- cspell:word fxpractice guéant lehalle navigations parameterizes -->

<!-- cspell:word raydium stablecorp stoikov tapia -->

# Market Making MVP — CADC/USDC

The **operating spec** for Dropset's first market-making vault: how a
single leader bot quotes CADC against USDC on the eCLOB at a 100 bps
spread with \$100 of inventory. It pins down the four numbers a bot
needs and nothing more — reference-price construction, the
`LiquidityProfile` ladder, update cadence, and the inventory/kill-switch
policy. The math is anchored to Avellaneda–Stoikov but the ladder is
hand-shaped: stable-pair σ is too small for the formal A-S skew to
matter at this size, so the bot uses A-S as a sanity check and a linear
override for inventory.

**Doc boundary.** Dependency flows one way: **`market-making-mvp.md` →
`architecture.md`**. This document references *down* into the protocol
spec (`LiquidityProfile`, `SetReferencePrice`, `SetLiquidityProfile`,
`FreezeVault`, flush math) and never the other way around. Nothing in
the protocol depends on this strategy — a different leader can run a
different shape against the same instructions.

**Objective.** Not yield. The MVP is a deliberately tiny live system
that exercises the leader interfaces end to end (price/profile cadence,
inventory drift, peg-deviation alarms) and lets us learn how the CADC
peg behaves against true FX before scaling capital. Once it survives, the
same shape re-parameterizes for other stablecoin-FX pairs.

**Scope.**

- Pair: CADC/USDC. CADC is Stablecorp's Canadian-dollar stablecoin;
  it tracks CAD/USD spot with a peg discount that is usually small but
  not zero.
- Vault TVL: **\$100 total max**, ~\$50 CADC + ~\$50 USDC at launch.
  Top-of-book size is bounded by leg inventory.
- Spread target: **100 bps quoted** at top of book (50 bps each side
  of mid). Holds for ~\$20 of one-sided trade; wider beyond.
- Single MM per market. No hedging, no shorting, no leverage.

______________________________________________________________________

## 1. Reference-price construction

### Sources

| Source                                                                           | Role                            | Notes                                                                                                                                                                                                                                                            |
| -------------------------------------------------------------------------------- | ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **CoinGecko** `coingecko.com/api/v3/simple/price?ids=cad-coin&vs_currencies=usd` | Primary CADC/USD                | Free tier 30 req/min; CoinGecko already aggregates exchanges, so this is itself a meta-feed.                                                                                                                                                                     |
| **Oanda Practice** `api-fxpractice.oanda.com/v3/instruments/USD_CAD/candles`     | FX sanity (true CAD/USD)        | API key required (free). Invert to CAD/USD. dropset-alpha already wraps this — reuse the [`oanda_price_feed.rs`](https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/oanda_price_feed.rs#L7) shape. |
| **Aerodrome (Base)** via GeckoTerminal pool API                                  | On-chain CADC/USDC ground truth | Where real CADC volume happens; reveals the peg discount in stablecoin land.                                                                                                                                                                                     |

These feeds measure **two different quantities**:

- CoinGecko CADC/USD and Aerodrome CADC/USDC both measure the CADC
  stablecoin's market price (in USD-ish units). These should agree
  tightly with each other.
- Oanda CAD/USD measures the underlying FX rate. It will diverge from
  the others by the CADC peg discount — sometimes tens of bps,
  occasionally more. **Oanda is a peg-bound sanity check, not a
  fair-mid input.**

### Composition

Let `c_cg` = CoinGecko CADC/USD, `c_ae` = Aerodrome CADC/USDC pool spot,
`f_fx` = Oanda CAD/USD.

```text
fair_mid = mean(c_cg, c_ae)     # the two CADC market-price sources
peg      = fair_mid / f_fx      # CADC's discount vs FX spot
```

- **Fair mid** is computed only from the two CADC sources, since they
  are the relevant quantity for quoting CADC vs USDC.
- If `|c_cg - c_ae| / fair_mid > 50 bps`, one of the CADC sources is
  stale or broken — pause `SetReferencePrice` and alert.
- If only one CADC source is fresh, use it but flag the vault as
  degraded (tighten kill switches by 50%).
- If both CADC sources are stale (> 5 min), stop quoting until at
  least one comes back.

### Peg sanity bound (Oanda)

`f_fx` is the only feed tracking actual CAD/USD. It is used solely to
detect a peg event:

```text
0.97 ≤ peg ≤ 1.03
```

Outside this band → freeze quotes via `FreezeVault` (peg event).
300 bps is generous for CADC, which has historically held within ~50 bps
of FX spot, and gives room for an FX gap on a Monday open without
spuriously freezing. Oanda staleness is non-fatal — it only disarms the
peg kill switch; quoting continues against the CADC sources.

### Polling cadence

| Feed      | Poll interval                                     |
| --------- | ------------------------------------------------- |
| CoinGecko | 10 s (well under 30 req/min free-tier ceiling)    |
| Oanda     | 15 s (M1 candles)                                 |
| Aerodrome | 10 s via GeckoTerminal, or per Base block via RPC |

Local `fair_mid` is recomputed on every poll; `SetReferencePrice` is
only called per the cadence rules in §3 — not on every poll.

______________________________________________________________________

## 2. Profile math

Relevant protocol facts (see **architecture.md → LiquidityProfile** and
**→ Flush**):

- `N_LEVELS = 8` bids + 8 asks per vault.
- Each `Level = { price_offset: Ppm32, size_bps: u16, expiry_offset: u32 }`.
- `price_offset` is ppm from `reference_price.price` — bids subtract,
  asks add.
- `size_bps` is fraction of the **inventory leg**: `quote_atoms` for
  bids (USDC), `base_atoms` for asks (CADC).
- **Invariant:** `Σ size_bps ≤ 10000` per side.
- Sizes auto-rescale to current inventory on each flush; the leader
  doesn't manage absolute atoms.

### Proposed ladder

Per side, symmetric at launch ($50 per leg, ~$100 vault, 50/50 split):

| Level | `price_offset` | bps from mid | `size_bps` | depth at launch | cumulative |
| ----- | -------------- | ------------ | ---------- | --------------- | ---------- |
| 1     | 5_000 ppm      | 50 bps       | 4000 (40%) | ~\$20           | \$20       |
| 2     | 10_000 ppm     | 100 bps      | 3000 (30%) | ~\$15           | \$35       |
| 3     | 20_000 ppm     | 200 bps      | 2000 (20%) | ~\$10           | \$45       |
| 4     | 50_000 ppm     | 500 bps      | 1000 (10%) | ~\$5            | \$50       |
| 5-8   | 0 / unused     | —            | 0          | —               | —          |

(Unit conversion: `1_000_000 ppm = 100% = 10_000 bps`, so **1 bp = 100 ppm**.)

Properties:

- `Σ size_bps = 10000` per side — fully commits the leg, no reserve.
- Top-to-top spread = `2 × 50 bps = 100 bps`.
- The first \$20 of one-sided trade clears at the 50 bps offset
  (100 bps spread).
- Beyond that, effective spread widens by level: the next $15 clears
  at a 100 bps half-spread, the next $10 at 200 bps, the final $5 at
  500 bps. Cumulative VWAP half-spread to clear the whole $50 leg is
  `(20·50 + 15·100 + 10·200 + 5·500)/50 = 140 bps`.

### Justification (Avellaneda–Stoikov)

The shape is hand-tuned but anchored to A-S
([Avellaneda & Stoikov 2008, *High-frequency trading in a limit order
book*][as2008]), equation (3.18) for the half-spread:

```text
half_spread = γ·σ²·τ/2 + (1/γ)·ln(1 + γ/κ)
```

For a CADC/USDC scale (realized daily vol ≈ 50 bps →
σ ≈ 5e-3 / √86_400 ≈ 1.7e-5 in price-units-per-√sec), small τ, and
γ = 0.1: the inventory term
`γσ²τ/2` is negligible; the half-spread is dominated by the
`(1/γ)·ln(1+γ/κ)` fill-intensity term, which with the dropset-alpha
defaults (κ from `FILL_DECAY_STEPS = 10`, `PRICE_STEP = 0.0001`) comes
out around **50 bps** — Level 1.

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
between legs means each side has moved $5 off the midpoint
($55 / $45 starting from $50 / $50), so the signed deviation is $5,
not $10.

For a $100 vault drifted by $10 (`q ≈ 5`), γ = 5 (more aggressive than
A-S default — small `Q` needs faster mean-reversion), σ ≈ 1.7e-5,
τ = 3600 s: `δ_ref` comes out sub-bps, which is too small to matter.

**Override with a linear inventory skew** instead: shift reference by
**5 bps per \$10 of deviation**, capped at ±20 bps. This is a hand-tuned
override of A-S because the stable-pair σ is so small that the formal
A-S skew is invisible at our size. Beyond a \$20 deviation, reshape the
ladder via `SetLiquidityProfile` (see §3).

______________________________________________________________________

## 3. Update cadence

### `SetReferencePrice` triggers (hot path)

`SetReferencePrice` is two aligned `u64` stores — the cheap path. Call
when **any** of:

1. `|fair_mid - last_set_price| / last_set_price > 10 bps` (price drift).
1. Heartbeat: 30 s elapsed since last set.
1. Inventory skew rule fires: `δ_ref_bps` changes by > 2 bps.

Expected: **2–6 calls per minute** in calm conditions. The
`quote_slot` argument can be pre-signed at an older slot if relay
latency matters (see **architecture.md → SetReferencePrice**;
`MAX_BACKDATE = 50 slots ≈ 20 s`).

### `SetLiquidityProfile` triggers (cold path)

`SetLiquidityProfile` rewrites the full ladder and arms a flush on the
next take. Call when **any** of:

1. Per-side inventory imbalance > 30% from launch ($65 / $35 or worse).
1. Realized σ over a 24 h window has doubled (vol-regime change).
1. Daily heartbeat (once per UTC day, fixed time).

Expected: **1–3 calls per day**.

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

**5-second tick.** Each tick: refresh feeds → recompute `fair_mid` →
evaluate triggers → fire at most one ix. No retry storms: if an ix
fails, skip the tick and retry on the next one.

### Fill detection

Hot-path ix emit nothing — see **architecture.md → Events and emission**.
The bot detects fills by subscribing to the `take` ix events emitted via
`emit_cpi!` (cold path, full fidelity, never dropped). A pre-flush state
diff is the fallback. **Do not poll account state for fills** — it is
too slow.

______________________________________________________________________

## 4. Inventory bounds & kill switches

| Trigger                                               | Action                                                                         |
| ----------------------------------------------------- | ------------------------------------------------------------------------------ |
| Imbalance > 30% from launch                           | Reshape: grow heavy side's `size_bps`, shift reference to invite rebalancing   |
| Imbalance > 50%                                       | Freeze heavy side (zero `size_bps` on that side; only the rebuild side quotes) |
| Imbalance > 80%                                       | `FreezeVault` — alert and review by hand                                       |
| Peg deviation outside `[0.97, 1.03]` of FX            | `FreezeVault`                                                                  |
| Any single feed > 5 min stale                         | Run degraded; tighten kill switches by 50%                                     |
| Both CADC sources disagreeing > 50 bps, or both stale | Stop calling `SetReferencePrice` until resolved                                |
| Vault TVL drops to \$80                               | `FreezeVault`, post-mortem                                                     |

`FreezeVault` is a leader ix from the protocol; everything else is
bot-side policy.

______________________________________________________________________

## 5. Explicitly deferred

- Full A-S optimization for finite `Q` ([Guéant 2017][gueant2017]) —
  hand-tuned ladder for MVP.
- Multi-oracle weighted composition across many sources (XE.com,
  Coinmarketcap, Jupiter, Raydium, Orca, Manifest, DFlow, Titan, …) —
  three uncorrelated sources are plenty for stable-pair sanity at MVP
  scale.
- Adversarial taker bot for hardening — separate effort.
- Hedging / shorting CADC for market-neutrality — separate effort,
  requires venue research.
- Performance fee / outside-depositor flow — comes with vault maturity,
  not MVP.
- Extension to other stablecoin-FX pairs — same shape, different σ and
  peg band; trivially re-parameterizes once CADC/USDC proves out.

[alpha-params]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/parameters.rs#L11
[alpha-spreads]: https://github.com/DASMAC-com/dropset-alpha/blob/fd16be56a72adf2e501b1310d85eb6519a10df5d/services/maker-bot/src/model/calculate_spreads.rs#L41
[as2008]: https://people.orie.cornell.edu/sfs33/LimitOrderBook.pdf
[gueant2011]: https://arxiv.org/abs/1105.3115
[gueant2017]: https://arxiv.org/abs/1605.01862
