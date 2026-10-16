/**
 * Pure share / NAV / PnL accounting kernels — the TypeScript mirror of
 * `dropset-math-core`'s `share` module (sdk/math-core/src/share.rs).
 *
 * A hand port of six of the seven Rust kernels — the seeding `isqrt`,
 * single-leg deposit sizing, the pro-rata withdrawal slice, the
 * performance-fee mint, realized-PnL crystallization, and the
 * shares-weighted cost-basis merge (`min_leader_share_ok` is on-chain only
 * and has no counterpart here). The Rust kernels **run on-chain**, so a
 * divergence here misreports consensus state. Nothing is shared with the
 * engine: this file is held to it only by the cross-language vectors in
 * `sdk/conformance/share_vectors.json` (see `share.conformance.test.ts`),
 * so behavior outside the pinned vectors is unguarded.
 *
 * All math is `bigint`, which subsumes the Rust `u128` / `i128`
 * intermediates — including the wide perf-fee numerator Rust divides
 * through a 256-bit `mul_div_floor` — and the helpers below stand in for
 * Rust's casts and saturating operators.
 */

import { quoteForBase, weightedAverage, type PriceBits } from './price';

const U64_MAX = (1n << 64n) - 1n;
const I64_MAX = (1n << 63n) - 1n;
const I64_MIN = -(1n << 63n);
const I128_MAX = (1n << 127n) - 1n;
/** Parts-per-million denominator for the perf-fee rate. Mirrors Rust `PPM`. */
const PPM = 1_000_000n;
/** Q32.32 fixed-point shift for value-per-share / high-water-mark. */
const Q32 = 32n;

/** `ceil(a / b)` for non-negative `bigint`s (mirrors Rust `u128::div_ceil`). */
function divCeil(a: bigint, b: bigint): bigint {
  return (a + b - 1n) / b;
}

/** Min of two `bigint`s. */
function min(a: bigint, b: bigint): bigint {
  return a < b ? a : b;
}

/** Max of two `bigint`s. */
function max(a: bigint, b: bigint): bigint {
  return a > b ? a : b;
}

/** Truncate to `u64` range, mirroring Rust's `as u64` cast (low 64 bits). */
function asU64(x: bigint): bigint {
  return x & U64_MAX;
}

/** Clamp into the signed `i64` range, mirroring Rust `i128::clamp(i64..)`. */
function clampI64(x: bigint): bigint {
  return x < I64_MIN ? I64_MIN : x > I64_MAX ? I64_MAX : x;
}

/**
 * Integer square root via Newton's method on `bigint` (mirrors Rust
 * `isqrt_u128`). Backs the seeding-deposit share formula
 * `total_shares := isqrt(base × quote)` and `Realize`'s `L = isqrt(base × quote)`.
 */
export function isqrtU128(n: bigint): bigint {
  if (n < 2n) return n;
  let x = n;
  let y = divCeil(x, 2n);
  while (y < x) {
    x = y;
    y = (x + n / x) / 2n;
  }
  return x;
}

/** Why a {@link singleLegBasket} sizing was rejected — mirrors Rust `BasketError`. */
export type BasketErrorKind =
  | 'SingleLegRequired'
  | 'MathOverflow'
  | 'BasketSlippage'
  | 'EmptyLeg'
  | 'ZeroShares';

export class BasketError extends Error {
  constructor(readonly kind: BasketErrorKind) {
    super(kind);
    this.name = 'BasketError';
  }
}

/** `(shares_out, base_in_final, quote_in_final)` from {@link singleLegBasket}. */
export type Basket = { sharesOut: bigint; baseInFinal: bigint; quoteInFinal: bigint };

/**
 * Single-leg subsequent-deposit sizing — spec invariant I1 (VPS-preserving),
 * mirroring Rust `single_leg_basket`. Exactly one leg is supplied
 * (`baseIn XOR quoteIn`); shares are **floored** and the basket is rounded
 * **up** so the vault never under-collects. Both finals are bounded by the
 * caller's `max*In` (else {@link BasketError} `BasketSlippage`). A supplied
 * leg the vault holds zero atoms of throws `EmptyLeg`; a deposit too small
 * for one whole share (or `totalShares == 0`) throws `ZeroShares`.
 */
export function singleLegBasket(
  totalShares: bigint,
  baseAtoms: bigint,
  quoteAtoms: bigint,
  baseIn: bigint,
  quoteIn: bigint,
  maxBaseIn: bigint,
  maxQuoteIn: bigint,
): Basket {
  if (baseIn > 0n === quoteIn > 0n) {
    throw new BasketError('SingleLegRequired');
  }
  const ts = totalShares;
  const b = baseAtoms;
  const q = quoteAtoms;
  const [legIn, legAtoms] = baseIn > 0n ? [baseIn, b] : [quoteIn, q];
  if (legAtoms === 0n) {
    throw new BasketError('EmptyLeg');
  }
  const sharesOut = (legIn * ts) / legAtoms;
  if (sharesOut === 0n) {
    throw new BasketError('ZeroShares');
  }
  if (sharesOut > U64_MAX) {
    throw new BasketError('MathOverflow');
  }
  const baseInFinal = divCeil(sharesOut * b, ts);
  const quoteInFinal = divCeil(sharesOut * q, ts);
  if (baseInFinal > maxBaseIn || quoteInFinal > maxQuoteIn) {
    throw new BasketError('BasketSlippage');
  }
  return { sharesOut, baseInFinal, quoteInFinal };
}

/**
 * Floored pro-rata basket slice for a withdrawal of `sharesIn` out of
 * `totalShares` (mirrors Rust `compute_pro_rata_slice`). Rounding **down**
 * keeps the dust in the vault for the remaining depositors. Caller's
 * precondition: `totalShares > 0`; an empty vault slices to `[0, 0]`.
 */
export function computeProRataSlice(
  sharesIn: bigint,
  totalShares: bigint,
  baseAtoms: bigint,
  quoteAtoms: bigint,
): [bigint, bigint] {
  if (totalShares === 0n) return [0n, 0n];
  const ts = totalShares;
  // Mirror Rust's truncating `as u64` cast on each slice. For valid inputs
  // (`sharesIn ≤ totalShares`) each slice is ≤ its atom leg, so the mask is a
  // no-op; it only bites if the precondition is violated, where it keeps the
  // two forks bit-identical instead of letting TS return an un-truncated value.
  return [asU64((sharesIn * baseAtoms) / ts), asU64((sharesIn * quoteAtoms) / ts)];
}

/** New share/HWM state from {@link realizePerfFee} — mirrors Rust `RealizeResult`. */
export type RealizeResult = {
  sharesMinted: bigint;
  hwmAfter: bigint;
  totalSharesAfter: bigint;
  leaderSharesAfter: bigint;
};

/**
 * Performance-fee accrual — the pure scalar core of the program's
 * `realize_in_place` (mirrors Rust `realize_perf_fee`). When
 * `VPS = L / totalShares` exceeds the high-water mark, mint `m` new shares
 * to the leader and bump HWM; otherwise no-op (still trailing HWM up to VPS
 * when there is excess but no fee). No-op when `totalShares == 0`, `L == 0`,
 * or `VPS <= HWM`. `L`, `HWM`, and intermediates are Q32.32.
 *
 * Mirrors the Rust guards exactly: a rate above `PPM` is clamped to `PPM`;
 * a zero denominator keeps the HWM advance; `m` is capped at the headroom
 * both share counts share below `u64::MAX`, so they move by the same delta;
 * and the stored HWM saturates into `u64` and never drops below `hwm`.
 */
export function realizePerfFee(
  baseAtoms: bigint,
  quoteAtoms: bigint,
  totalShares: bigint,
  leaderShares: bigint,
  hwm: bigint,
  perfFeeRate: bigint,
): RealizeResult {
  const s = totalShares;
  const noop: RealizeResult = {
    sharesMinted: 0n,
    hwmAfter: hwm,
    totalSharesAfter: s,
    leaderSharesAfter: leaderShares,
  };
  if (s === 0n) return noop;
  const fPpm = min(perfFeeRate, PPM);
  // Two u64 atom counts multiplied fit in u128, so no saturation needed.
  const l = isqrtU128(baseAtoms * quoteAtoms);
  if (l === 0n) return noop;
  const vps = (l << Q32) / s;
  if (vps <= hwm) return noop;
  const advanced: RealizeResult = { ...noop, hwmAfter: ratchetHwm(vps, hwm) };
  if (fPpm === 0n) return advanced;
  // m = f · s · (L − hwm·s) / ((1 − f) · L + f · hwm·s); the `hwm·s` term is
  // shifted back from Q32.32 to atom scale before the division.
  const hwmS = (hwm * s) >> Q32;
  if (l <= hwmS) return advanced;
  const num = fPpm * s * (l - hwmS);
  const oneMinusF = PPM - fPpm;
  const denom = oneMinusF * l + fPpm * hwmS;
  if (denom === 0n) return advanced;
  const headroom = min(U64_MAX - s, max(U64_MAX - leaderShares, 0n));
  const m = min(num / denom, headroom);
  if (m === 0n) return advanced;
  const sAfter = s + m;
  return {
    sharesMinted: m,
    hwmAfter: ratchetHwm((l << Q32) / sAfter, hwm),
    totalSharesAfter: sAfter,
    leaderSharesAfter: leaderShares + m,
  };
}

/** Saturate a Q32.32 VPS into `u64` and floor it at `hwm` (Rust `ratchet_hwm`). */
function ratchetHwm(vps: bigint, hwm: bigint): bigint {
  return max(min(vps, U64_MAX), hwm);
}

/** Why a {@link crystallizePnl} call failed — mirrors Rust `CrystallizeError`. */
export type CrystallizeErrorKind = 'InsufficientShares' | 'MathOverflow';

export class CrystallizeError extends Error {
  constructor(readonly kind: CrystallizeErrorKind) {
    super(kind);
    this.name = 'CrystallizeError';
  }
}

/** New cost-basis state from {@link crystallizePnl} — mirrors Rust `CrystallizeResult`. */
export type CrystallizeResult = {
  realizedPrice: bigint;
  realizedYield: bigint;
  realizedPnl: bigint;
  sharesAfter: bigint;
  netDepositsAfter: bigint;
  pnlDelta: bigint;
};

/**
 * Crystallize the realized PnL of withdrawing `sharesIn` shares delivering
 * the `(sliceBase, sliceQuote)` basket — the pure scalar core of the
 * program's `crystallize_realized_pnl` (mirrors Rust `crystallize_pnl`):
 *
 *   realized_price    += slice_base × (ref_now − entry_ref)
 *   realized_yield += slice_quote + slice_base × entry_ref − released_basis
 *   realized_pnl   += slice_quote + slice_base × ref_now  − released_basis
 *
 * `ref × slice_base` is decoded via {@link quoteForBase} (quote-atom units).
 * Signed accumulators clamp into `i64`. An empty position (`shares == 0`)
 * throws `InsufficientShares` for any `sharesIn`, zero included.
 */
export function crystallizePnl(
  sharesIn: bigint,
  shares: bigint,
  netDeposits: bigint,
  sliceBase: bigint,
  sliceQuote: bigint,
  entryRefPrice: PriceBits,
  refNow: PriceBits,
  realizedPrice: bigint,
  realizedYield: bigint,
  realizedPnl: bigint,
): CrystallizeResult {
  if (shares === 0n || shares < sharesIn) {
    throw new CrystallizeError('InsufficientShares');
  }
  // `netDeposits × sharesIn` is a product of two u64 — fits in u128, so the
  // Rust `checked_mul` never trips here.
  const releasedBasis = (netDeposits * sharesIn) / shares;
  const quoteForRefNow = min(quoteForBase(refNow, sliceBase), I128_MAX);
  const quoteForRefEntry = min(quoteForBase(entryRefPrice, sliceBase), I128_MAX);
  const fxDelta = quoteForRefNow - quoteForRefEntry;
  const yieldDelta = sliceQuote + quoteForRefEntry - releasedBasis;
  const pnlDelta = sliceQuote + quoteForRefNow - releasedBasis;
  const newFx = clampI64(realizedPrice + fxDelta);
  const newPnl = clampI64(realizedPnl + pnlDelta);
  const newYield = clampI64(realizedYield + yieldDelta);
  // released_basis ≤ net_deposits since sharesIn ≤ shares, but mirror the
  // Rust `checked_sub` guard rather than assume it.
  if (releasedBasis > netDeposits) {
    throw new CrystallizeError('MathOverflow');
  }
  return {
    realizedPrice: newFx,
    realizedYield: newYield,
    realizedPnl: newPnl,
    sharesAfter: shares - sharesIn,
    netDepositsAfter: netDeposits - releasedBasis,
    pnlDelta: clampI64(pnlDelta),
  };
}

/**
 * Shares-weighted merge of a depositor's entry basis on a top-off deposit —
 * the pure scalar core of `VaultDepositorHeader::record_deposit`'s top-off
 * arm (mirrors Rust `merge_entry_basis`). `entryVps` is Q32.32 (a raw
 * weighted average is exact); `entryRef` routes through
 * {@link weightedAverage}. Returns the merged `(entryVpsNew, entryRefNew)`,
 * or the prior basis unchanged when both weights are zero.
 */
export function mergeEntryBasis(
  priorShares: bigint,
  sharesOut: bigint,
  entryVpsPrev: bigint,
  vpsAfter: bigint,
  entryRefPrev: PriceBits,
  refNow: PriceBits,
): [bigint, PriceBits] {
  const s = priorShares;
  const ds = sharesOut;
  const denom = s + ds;
  if (denom === 0n) return [entryVpsPrev, entryRefPrev];
  // `entryVpsNew` is a weighted average of two u64 VPS values, so ≤ u64::MAX
  // for valid inputs; `asU64` mirrors Rust's `as u64` cast (identity here).
  const entryVpsNew = asU64((s * entryVpsPrev + ds * vpsAfter) / denom);
  const entryRefNew = weightedAverage(entryRefPrev, refNow, s, ds);
  return [entryVpsNew, entryRefNew];
}
