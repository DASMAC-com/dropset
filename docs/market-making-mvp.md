<!-- cspell:word illiquidity -->

<!-- cspell:word guéant -->

<!-- cspell:word lehalle -->

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

### Tiered sources (with failover)

Each market's USD reference cascades through four sources, primary-first.
A tier that errors, times out, or goes stale fails over to the next; the
bot surfaces which tier is live per market.

| Tier | Source                                                       | Role                | Notes                                                                                                              |
| ---- | ------------------------------------------------------------ | ------------------- | ------------------------------------------------------------------------------------------------------------------ |
| 1    | **CoinGecko** `/api/v3/simple/price?ids=…&vs_currencies=usd` | Primary token/USD   | One batched call prices every market. Free tier ~30 req/min; CoinGecko already aggregates exchanges (a meta-feed). |
| 2    | **CoinMarketCap** `/v2/cryptocurrency/quotes/latest?id=…`    | Secondary token/USD | Batched by numeric id, `X-CMC_PRO_API_KEY` from env. Free-tier ~10k/mo quota → polled only on primary failure.     |
| 3    | **ECB/Frankfurter** `/latest?base=USD&symbols=…`             | Keyless FX peg rate | `<ccy>` per USD, inverted to USD per unit — a pure peg rate, not a market price. ECB publishes once a working day. |
| 4    | **Static**                                                   | Last-resort peg     | A per-market constant spot value, used when every live source is down.                                             |

The tiers measure **two different quantities**:

- CoinGecko and CoinMarketCap measure the token's **market price** in
  USD. These are the quoting mid when live.
- Frankfurter and the static fallback are the underlying **FX peg rate**.
  For a sound peg these sit within a few bps of the market price; when a
  market tier is down they are the best estimate the bot has, but they
  carry no independent market signal.

### Composition

For one market, the highest live tier anchors the mid:

```text
mid  = first live of (coingecko, coinmarketcap, fx_rate, static)
peg  = mid / fx_rate    # only when a market tier AND a fresh FX rate exist
```

- A live **market price** (tier 1 or 2) quotes healthy.
- A **peg-rate fallback** (tier 3 or 4) has no live market price, so the
  vault runs **degraded**: quoting continues off the peg, but the kill
  switches tighten by 50% (§4 row 5). The "full degrade" the failover
  calls out — every live source down to the static peg — is the deepest
  case of this.
- When a market price and a fresh FX rate coexist, `peg` cross-checks the
  token against its fiat (`≈ 1` for a sound peg); the peg-rate tiers have
  nothing independent to check against themselves, so they carry no peg.

### Peg sanity bound

When `peg` exists it gates a freeze:

```text
0.97 ≤ peg ≤ 1.03
```

Outside this band → halt quotes (peg event). 300 bps is generous for
these stablecoins, which historically hold within ~50 bps of FX spot,
and gives room for an FX gap on a Monday open without spuriously
freezing. A missing FX rate is non-fatal — it only disarms the peg
switch; quoting continues against the market price.

### Polling cadence

| Tier            | Poll interval                                             |
| --------------- | --------------------------------------------------------- |
| CoinGecko       | 10 s (one batched call, well under the free-tier ceiling) |
| CoinMarketCap   | ≥ 60 s, and only while CoinGecko is down (quota guard)    |
| ECB/Frankfurter | 300 s (ECB publishes daily; a slow poll suffices)         |

Local `mid` is recomputed every tick; `SetReferencePrice` is only called
per the cadence rules in §3 — not on every poll.

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

| Trigger                                       | Action                                                                         |
| --------------------------------------------- | ------------------------------------------------------------------------------ |
| Imbalance > 30% from launch                   | Reshape: shrink the accumulating side so the heavy side dominates and offloads |
| Imbalance > 50%                               | Freeze heavy side (zero `size_bps` on that side; only the rebuild side quotes) |
| Imbalance > 80%                               | `FreezeVault` — alert and review by hand                                       |
| Peg deviation outside `[0.97, 1.03]` of FX    | `FreezeVault`                                                                  |
| No live market price (peg-rate fallback only) | Run degraded; tighten kill switches by 50%                                     |
| Every live source down → static peg           | Full degrade (the deepest degraded case)                                       |
| Vault TVL drops below 80% of launch TVL       | `FreezeVault`, post-mortem                                                     |

`FreezeVault` is admin-only and irreversible, so the bot maps these hard
triggers to a leader-authorized halt (zero the profile, let levels
expire, alert) rather than calling `FreezeVault` autonomously; a real
freeze stays a human decision. Per-market TVL-floor and skew calibration
is coordinated separately.

______________________________________________________________________

## 5. Explicitly deferred

- Full A-S optimization for finite `Q` ([Guéant 2017][gueant2017]) —
  hand-tuned ladder for MVP.
- Weighted multi-oracle composition across many simultaneous sources
  (Jupiter, Raydium, Orca, Manifest, DFlow, …) — the tiered cascade uses
  one source at a time, failing over rather than blending.
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
