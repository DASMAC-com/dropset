/**
 * Native-vs-relative quoting (TypeScript mirror of `dropset-sdk`'s
 * `quoting` module).
 *
 * The program quotes *relatively*: a single reference price plus a
 * `LiquidityProfile` of per-level ppm offsets and bps sizes. This module
 * adds the **native CLOB** direction — specify a full book of absolute
 * price levels and atom sizes, and {@link nativeBookToProfileBytes}
 * translates it into the 160-byte `profile_bytes` arg that
 * `getSetLiquidityProfileInstruction` expects.
 *
 * The translation is the inverse of the on-chain flush: an ask at
 * absolute price `P` against reference `R` becomes a ppm offset
 * `(P/R - 1)·1e6`; a level of `size` atoms against an inventory leg of
 * `leg` atoms becomes `size/leg·10000` bps. Per-side `Σ size_bps ≤ 10000`,
 * matching the invariant the program enforces at `set_liquidity_profile`.
 */

import { isInfinityPrice, isZeroPrice, quoteForBase, type PriceBits } from './price';

/** Bid/ask levels per side (matches the program's `N_LEVELS`). */
export const N_LEVELS = 8;
const PPM = 1_000_000n;
const BPS = 10_000n;
/** Common scale for the integer price ratio — mirrors Rust's `SCALE`. */
const SCALE = 1_000_000_000n;
/** Serialized `LiquidityProfile` length: 2 sides × 8 levels × 10 bytes. */
export const PROFILE_BYTES = 2 * N_LEVELS * 10;

/** One level of a native (absolute-price) book. */
export type NativeLevel = {
  /** Absolute price as raw `Price` bits (see {@link encodePrice}). */
  price: PriceBits;
  /** Allowance in atoms: base atoms for asks, quote atoms for bids. */
  size: bigint;
  /** Per-level expiry, in slots after the reference's `quote_slot`. */
  expiryOffset: number;
};

/** A native book: absolute-price ladders, top-of-book first, ≤ 8 per side. */
export type NativeBook = {
  bids: NativeLevel[];
  asks: NativeLevel[];
};

export class QuotingError extends Error {}

type RelLevel = { priceOffset: number; sizeBps: number; expiryOffset: number };

function levelToRelative(
  lvl: NativeLevel,
  legAtoms: bigint,
  reference: PriceBits,
  isAsk: boolean,
): RelLevel {
  if (isZeroPrice(reference) || isInfinityPrice(reference)) {
    throw new QuotingError('reference price is not a regular price');
  }
  // ratioPpm = (P / R) · 1e6 in exact integer math — byte-identical to the
  // Rust path (`quote_for_base(SCALE)` then `p_val * PPM / ref_val`), so
  // both SDKs emit the same profile_bytes. Float division here would
  // diverge by up to 1 ppm at boundaries.
  const refVal = quoteForBase(reference, SCALE);
  if (refVal === 0n) throw new QuotingError('reference price is not a regular price');
  const ratioPpm = (quoteForBase(lvl.price, SCALE) * PPM) / refVal;
  let offset: bigint;
  if (isAsk) {
    if (ratioPpm < PPM) throw new QuotingError('ask priced at or below reference');
    offset = ratioPpm - PPM;
  } else {
    if (ratioPpm > PPM) throw new QuotingError('bid priced at or above reference');
    offset = PPM - ratioPpm;
  }
  if (offset > 0xffff_ffffn) throw new QuotingError('price offset overflows u32');
  if (legAtoms <= 0n) throw new QuotingError('inventory leg is zero');
  const sizeBps = (lvl.size * BPS) / legAtoms;
  if (sizeBps > BPS) throw new QuotingError('level size exceeds inventory leg');
  return { priceOffset: Number(offset), sizeBps: Number(sizeBps), expiryOffset: lvl.expiryOffset };
}

function writeLevel(view: DataView, offset: number, l: RelLevel): void {
  view.setUint32(offset, l.priceOffset, true);
  view.setUint16(offset + 4, l.sizeBps, true);
  view.setUint32(offset + 6, l.expiryOffset, true);
}

/**
 * Translate a native book into the `[u8; 160]` `profile_bytes` arg for
 * `set_liquidity_profile`, anchored to `reference` and sized against the
 * vault's current `(baseAtoms, quoteAtoms)`. Ask sizes are fractions of
 * `baseAtoms`, bid sizes fractions of `quoteAtoms`.
 */
export function nativeBookToProfileBytes(
  book: NativeBook,
  reference: PriceBits,
  baseAtoms: bigint,
  quoteAtoms: bigint,
): Uint8Array {
  if (book.bids.length > N_LEVELS || book.asks.length > N_LEVELS) {
    throw new QuotingError(`more than ${N_LEVELS} levels on a side`);
  }
  const bytes = new Uint8Array(PROFILE_BYTES);
  const view = new DataView(bytes.buffer);
  const levelBytes = 10;

  // Layout: bids[0..8] then asks[0..8].
  let bidBps = 0;
  book.bids.forEach((lvl, i) => {
    const rel = levelToRelative(lvl, quoteAtoms, reference, false);
    bidBps += rel.sizeBps;
    writeLevel(view, i * levelBytes, rel);
  });
  let askBps = 0;
  book.asks.forEach((lvl, i) => {
    const rel = levelToRelative(lvl, baseAtoms, reference, true);
    askBps += rel.sizeBps;
    writeLevel(view, (N_LEVELS + i) * levelBytes, rel);
  });
  if (bidBps > 10_000 || askBps > 10_000) {
    throw new QuotingError('per-side Σ size_bps exceeds 10000');
  }
  return bytes;
}
