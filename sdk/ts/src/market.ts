/**
 * On-chain market-account reader: decode the market slab and reconstruct
 * the resting order book.
 *
 * TypeScript port of the Rust `dropset-interface` crate (`layout.rs` +
 * `matching.rs`) — the reusable on-chain read primitive behind the demo
 * order-book viz. A market is a single account, so a live poll is one
 * `getAccountInfo`: {@link fetchDropsetMarketView} fetches it (plus the
 * current slot, for expiry filtering) and returns `{ header, bids, asks }`.
 *
 * The account is stored as `Slab<MarketHeader, Vault>`: an 8-byte Anchor
 * discriminator, a fixed `MarketHeader`, a `u32` slab length, alignment
 * padding, then a tail of fixed-size `Vault` sectors. The `Vault` slab is
 * opaque to the IDL, so the generated client can't decode it — this module
 * mirrors the byte layout (offsets track `sdk/interface/src/layout.rs`) and
 * reuses the generated `MarketHeader` codec for the header.
 *
 * Book reconstruction walks the active doubly-linked list from
 * `header.head`, materializes each vault's live levels (from the
 * `LiquidityProfile` when a flush is armed, else from stored `remaining`
 * state), then sorts cross-vault by price-time priority — the same book the
 * on-chain matcher fills against (ports `collect_side_levels` /
 * `level_state` / `resting_levels`).
 */

import {
  assertAccountExists,
  fetchEncodedAccount,
  type Address,
  type FetchAccountConfig,
  type ReadonlyUint8Array,
} from '@solana/kit';

import { getMarketHeaderDecoder, getMarketHeaderSize, type MarketHeader } from './generated';
// `N_LEVELS` is the protocol's per-side ladder depth, shared with the
// native-book quoting encoder — import it rather than redeclare it.
import { N_LEVELS } from './quoting';
import {
  baseForQuote,
  fromScaled,
  isInfinityPrice,
  isValidPrice,
  isZeroPrice,
  priceBiasedExponent,
  priceBidKey,
  priceSignificand,
  PRICE_ZERO,
  type PriceBits,
} from './price';

const PPM = 1_000_000n;
const BPS = 10_000n;

/** Sentinel for sector-index pointers (`head`, `next`, `prev`). */
export const NULL_SECTOR = 0xffff_ffff;
/** Flush flag OR'd onto a vault's `reference_price.stamp` (bit 63). */
const FLUSH_BIT = 1n << 63n;
/** On-chain `align_of::<Vault>()` — the slab aligns the first sector to it. */
const VAULT_ALIGN = 4;
/** `size_of::<Vault>()`: the sector stride in the slab tail. */
export const VAULT_SIZE = 692;

// ── Vault field byte offsets (mirror `layout.rs` `Vault`) ─────────────
const V_NEXT = 0;
const V_REF_STAMP = 72;
const V_REF_PRICE = 80;
const V_REF_QUOTE_SLOT = 84;
const V_REF_QUOTE_UNIX = 88;
const V_BASE_ATOMS = 92;
const V_QUOTE_ATOMS = 100;
const V_FROZEN = 140;
const V_PROFILE = 148; // LiquidityProfile: bids[N] then asks[N]
const V_REMAINING = 372; // Remaining: bids[N] then asks[N]

// Level: price_offset u32, size_bps u16, expiry_offset_secs u32,
// expiry_offset_slots u32 — all alignment-1, so byte-packed.
const LEVEL_SIZE = 14;
// Position: price u32, size u64, expires_at_unix u32, expires_at_slot u32.
const POSITION_SIZE = 20;
const SIDE_LEVELS_BYTES = N_LEVELS * LEVEL_SIZE;
const SIDE_POSITIONS_BYTES = N_LEVELS * POSITION_SIZE;

/** One relative level of a vault's `LiquidityProfile` (a flush ladder rung). */
type ProfileLevel = {
  priceOffset: number;
  sizeBps: number;
  expiryOffsetSecs: number;
  expiryOffsetSlots: number;
};

/** One stored `remaining` book level: absolute price, size, and expiry. */
type RemainingPosition = {
  price: PriceBits;
  size: bigint;
  expiresAtUnix: number;
  expiresAtSlot: number;
};

/** A decoded `Vault` sector — only the fields the book reader needs. */
export type VaultView = {
  /** Next sector in the active DLL, or {@link NULL_SECTOR}. */
  next: number;
  /** `reference_price.stamp`: the price-time nonce with the flush bit. */
  referenceStamp: bigint;
  /** `reference_price.price` as raw {@link PriceBits}. */
  referencePrice: PriceBits;
  /** `reference_price.quote_slot`: the datum flush slot-expiries are
   * measured from. */
  quoteSlot: number;
  /** `reference_price.quote_unix`: the wall-clock datum, in unix seconds,
   * that flush expiries are measured from. */
  quoteUnix: number;
  /** Pooled base inventory in atoms. */
  baseAtoms: bigint;
  /** Pooled quote inventory in atoms. */
  quoteAtoms: bigint;
  /** Non-zero when the vault is frozen (skipped from matching). */
  frozen: number;
  profileBids: ProfileLevel[];
  profileAsks: ProfileLevel[];
  remainingBids: RemainingPosition[];
  remainingAsks: RemainingPosition[];
};

/** A decoded market account: the header plus every slab sector. */
export type MarketSlab = { header: MarketHeader; sectors: VaultView[] };

/** A resting level: an absolute `price` and its depth in **base atoms**. */
export type BookLevel = { price: PriceBits; size: bigint };

/** The reconstructed book: the header and both sides, best price first. */
export type DropsetMarketView = { header: MarketHeader; bids: BookLevel[]; asks: BookLevel[] };

/** Raised when the raw account bytes can't be decoded as a market slab. */
export class MarketLayoutError extends Error {}

// ── Slab decode (mirror `MarketView::load`) ──────────────────────────

function readProfileLevels(dv: DataView, base: number): ProfileLevel[] {
  const out: ProfileLevel[] = [];
  for (let i = 0; i < N_LEVELS; i++) {
    const o = base + i * LEVEL_SIZE;
    out.push({
      priceOffset: dv.getUint32(o, true),
      sizeBps: dv.getUint16(o + 4, true),
      expiryOffsetSecs: dv.getUint32(o + 6, true),
      expiryOffsetSlots: dv.getUint32(o + 10, true),
    });
  }
  return out;
}

function readRemainingPositions(dv: DataView, base: number): RemainingPosition[] {
  const out: RemainingPosition[] = [];
  for (let i = 0; i < N_LEVELS; i++) {
    const o = base + i * POSITION_SIZE;
    out.push({
      price: dv.getUint32(o, true),
      size: dv.getBigUint64(o + 4, true),
      expiresAtUnix: dv.getUint32(o + 12, true),
      expiresAtSlot: dv.getUint32(o + 16, true),
    });
  }
  return out;
}

function decodeVault(dv: DataView, base: number): VaultView {
  return {
    next: dv.getUint32(base + V_NEXT, true),
    referenceStamp: dv.getBigUint64(base + V_REF_STAMP, true),
    referencePrice: dv.getUint32(base + V_REF_PRICE, true),
    quoteSlot: dv.getUint32(base + V_REF_QUOTE_SLOT, true),
    quoteUnix: dv.getUint32(base + V_REF_QUOTE_UNIX, true),
    baseAtoms: dv.getBigUint64(base + V_BASE_ATOMS, true),
    quoteAtoms: dv.getBigUint64(base + V_QUOTE_ATOMS, true),
    frozen: dv.getUint8(base + V_FROZEN),
    profileBids: readProfileLevels(dv, base + V_PROFILE),
    profileAsks: readProfileLevels(dv, base + V_PROFILE + SIDE_LEVELS_BYTES),
    remainingBids: readRemainingPositions(dv, base + V_REMAINING),
    remainingAsks: readRemainingPositions(dv, base + V_REMAINING + SIDE_POSITIONS_BYTES),
  };
}

/**
 * Decode a market account's full data buffer (including the 8-byte
 * discriminator) into its header and slab sectors. Mirrors
 * `MarketView::load`; throws {@link MarketLayoutError} on a buffer too
 * short for the header + slab length, or a slab length that overruns it.
 */
export function decodeMarketSlab(data: ReadonlyUint8Array): MarketSlab {
  // `getMarketHeaderSize()` is the discriminator + header length; the
  // `u32` slab length sits immediately after it.
  const lenAt = getMarketHeaderSize();
  if (data.length < lenAt + 4) {
    throw new MarketLayoutError('account buffer too small for header + slab length');
  }
  const header = getMarketHeaderDecoder().decode(data.subarray(0, lenAt));
  const dv = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const len = dv.getUint32(lenAt, true);
  // The slab aligns the first sector to the on-chain Vault align; the
  // stride is a multiple of it so later sectors stay aligned.
  const itemsStart = (lenAt + 4 + VAULT_ALIGN - 1) & ~(VAULT_ALIGN - 1);
  const end = itemsStart + len * VAULT_SIZE;
  if (data.length < end) {
    throw new MarketLayoutError('slab length exceeds account buffer');
  }
  const sectors: VaultView[] = [];
  for (let i = 0; i < len; i++) {
    sectors.push(decodeVault(dv, itemsStart + i * VAULT_SIZE));
  }
  return { header, sectors };
}

// ── Book reconstruction (mirror `matching.rs`) ───────────────────────

/**
 * Materialize an absolute-price from a reference price and a ppm offset.
 * Asks: `ref × (PPM + offset) / PPM`; bids: `ref × max(PPM − offset, 0) /
 * PPM`. Sentinels pass through; an offset that zeroes the factor (or a
 * result outside the exponent range) collapses to {@link PRICE_ZERO},
 * which the level filter then drops. Mirrors `matching_math::flush_level_price`.
 */
function flushLevelPrice(reference: PriceBits, offsetPpm: number, isAsk: boolean): PriceBits {
  if (isZeroPrice(reference) || isInfinityPrice(reference)) return reference;
  const sig = BigInt(priceSignificand(reference));
  const exp = priceBiasedExponent(reference);
  const off = BigInt(offsetPpm >>> 0);
  const factor = isAsk ? PPM + off : off >= PPM ? 0n : PPM - off;
  if (factor === 0n) return PRICE_ZERO;
  const scaled = (sig * factor) / PPM;
  try {
    return fromScaled(scaled, exp);
  } catch {
    return PRICE_ZERO;
  }
}

/**
 * A level's materialized size in atoms: `size_bps` of the matching
 * inventory leg. Returns `null` when `size_bps > BPS` — the corrupt-bytes
 * case for which a collected side is skipped whole (see
 * `flushSideSumExceedsBps`), so on a materialized side this is never `null`.
 * Mirrors `matching_math::level_fill_atoms`.
 */
function levelFillAtoms(sizeBps: number, legAtoms: bigint): bigint | null {
  if (BigInt(sizeBps) > BPS) return null;
  return (legAtoms * BigInt(sizeBps)) / BPS;
}

/** Cross-vault sort key: raw bits for asks (cheapest first), inverted for bids. */
function sortKey(price: PriceBits, isAsk: boolean): number {
  return isAsk ? price >>> 0 : priceBidKey(price);
}

/**
 * True when the flush profile's collected side (`isAsk` ⇒ asks) sums past
 * `BPS` — `Σ size_bps > BPS`, which subsumes any single level `> BPS`. The
 * on-chain matcher zeroes such a side's `remaining` at flush time, dropping
 * it from matching without aborting the take, so the simulator mirrors that
 * by skipping the vault's contribution on the collected side. Mirrors
 * `matching::flush_side_sum_exceeds_bps`.
 */
function flushSideSumExceedsBps(v: VaultView, isAsk: boolean): boolean {
  const side = isAsk ? v.profileAsks : v.profileBids;
  let sum = 0;
  for (let i = 0; i < N_LEVELS; i++) sum += side[i]!.sizeBps;
  return BigInt(sum) > BPS;
}

/** A live, matchable level pulled from a vault during book construction. */
type Lvl = {
  key: number;
  price: PriceBits;
  nonce: bigint;
  sector: number;
  level: number;
  size: bigint;
};

/**
 * Resolve one level's `(price, size, expiresAtUnix)`: materialize from the
 * `LiquidityProfile` when a flush is armed, else read stored `remaining`
 * state. Mirrors `matching::level_state`.
 */
function levelState(
  v: VaultView,
  i: number,
  isAsk: boolean,
  flush: boolean,
  reference: PriceBits,
  refSlot: number,
  refUnix: number,
): { price: PriceBits; size: bigint; expiresAtUnix: number; expiresAtSlot: number } {
  if (flush) {
    const lvl = (isAsk ? v.profileAsks[i] : v.profileBids[i])!;
    const price = flushLevelPrice(reference, lvl.priceOffset, isAsk);
    const leg = isAsk ? v.baseAtoms : v.quoteAtoms;
    // A side that sums past BPS is skipped whole by the
    // `flushSideSumExceedsBps` gate in `collectSideLevels` before this runs,
    // so on a collected side every level is `≤ Σ ≤ BPS` and `?? 0n` is an
    // unreachable total-function fallback, not a silent level drop.
    const size = levelFillAtoms(lvl.sizeBps, leg) ?? 0n;
    return {
      price,
      size,
      expiresAtUnix: deadline(refUnix, lvl.expiryOffsetSecs),
      expiresAtSlot: deadline(refSlot, lvl.expiryOffsetSlots),
    };
  }
  const p = (isAsk ? v.remainingAsks[i] : v.remainingBids[i])!;
  return {
    price: p.price,
    size: p.size,
    expiresAtUnix: p.expiresAtUnix,
    expiresAtSlot: p.expiresAtSlot,
  };
}

/**
 * One domain's absolute deadline, mirroring the on-chain `Vault::deadline`:
 * a saturating add, except that a **zero offset yields zero** — the dead
 * sentinel — so "no life in this domain" survives the addition instead of
 * collapsing onto the bare datum.
 */
function deadline(datum: number, offset: number): number {
  return offset === 0 ? 0 : Math.min(datum + offset, NULL_SECTOR);
}

/** Whether the active DLL cycles or points out of bounds (mirror `active_dll_is_corrupt`). */
function activeDllIsCorrupt(slab: MarketSlab): boolean {
  const n = slab.sectors.length;
  let cur = slab.header.head;
  let steps = n;
  while (cur !== NULL_SECTOR) {
    if (steps === 0) return true;
    steps -= 1;
    if (cur >= n) return true;
    cur = slab.sectors[cur]!.next;
  }
  return false;
}

/** Walk the active DLL from `header.head`, yielding `[sector, vault]` pairs. */
function activeVaults(slab: MarketSlab): Array<[number, VaultView]> {
  const out: Array<[number, VaultView]> = [];
  const n = slab.sectors.length;
  let cur = slab.header.head;
  let steps = n;
  while (cur !== NULL_SECTOR && steps > 0) {
    steps -= 1;
    if (cur >= n) break; // bad pointer — end the walk (already flagged as corrupt)
    const v = slab.sectors[cur]!;
    out.push([cur, v]);
    cur = v.next;
  }
  return out;
}

/**
 * Collect one side's live, matchable levels across all active vaults,
 * sorted into cross-vault price-time priority: best price first, then
 * older quote (lower nonce), then lower sector, then lower level. Returns
 * `null` only when the book is in a state the on-chain engine hard-rejects
 * (a corrupt active DLL). An oversized flush side (`Σ size_bps > BPS`) is
 * *not* a hard reject: the engine zeroes that side's `remaining` so it
 * contributes nothing while the rest of the book still matches, and this
 * collector mirrors that by skipping the offending vault's contribution on
 * the collected side. Mirrors `matching::collect_side_levels`.
 */
function collectSideLevels(
  slab: MarketSlab,
  isAsk: boolean,
  nowSlot: number,
  nowUnix: number,
): Lvl[] | null {
  if (activeDllIsCorrupt(slab)) return null;

  const levels: Lvl[] = [];
  for (const [sector, v] of activeVaults(slab)) {
    const reference = v.referencePrice;
    // Skip vaults the matcher won't touch: invalid/sentinel ref price or frozen.
    if (
      !isValidPrice(reference) ||
      isZeroPrice(reference) ||
      isInfinityPrice(reference) ||
      v.frozen !== 0
    ) {
      continue;
    }
    const nonce = v.referenceStamp & ~FLUSH_BIT;
    const flush = (v.referenceStamp & FLUSH_BIT) !== 0n;
    if (flush && flushSideSumExceedsBps(v, isAsk)) continue;
    const refUnix = v.quoteUnix;
    const refSlot = v.quoteSlot;

    for (let i = 0; i < N_LEVELS; i++) {
      const { price, size, expiresAtUnix, expiresAtSlot } = levelState(
        v,
        i,
        isAsk,
        flush,
        reference,
        refSlot,
        refUnix,
      );
      // Both conjuncts, exactly as the engine gates them.
      if (
        size === 0n ||
        expiresAtUnix <= nowUnix ||
        expiresAtSlot <= nowSlot ||
        isZeroPrice(price) ||
        isInfinityPrice(price) ||
        !isValidPrice(price)
      ) {
        continue;
      }
      levels.push({ key: sortKey(price, isAsk), price, nonce, sector, level: i, size });
    }
  }

  levels.sort(
    (a, b) =>
      a.key - b.key ||
      (a.nonce < b.nonce ? -1 : a.nonce > b.nonce ? 1 : 0) ||
      a.sector - b.sector ||
      a.level - b.level,
  );
  return levels;
}

/**
 * Reconstruct the resting levels on one `side` at `(nowSlot, nowUnix)` in
 * cross-vault price-time priority (best price first). Each level's `size`
 * is normalized to **base atoms** — an ask carries base atoms directly, a
 * bid's matchable quote leg is converted to base at the level price — so
 * both sides are comparable. Returns `[]` for an empty or engine-rejected
 * book. Mirrors `matching::resting_levels`.
 */
export function restingLevels(
  slab: MarketSlab,
  side: 'bid' | 'ask',
  nowSlot: number | bigint,
  nowUnix: number | bigint,
): BookLevel[] {
  const isAsk = side === 'ask';
  const levels = collectSideLevels(slab, isAsk, Number(nowSlot), Number(nowUnix));
  if (levels === null) return [];
  return levels.map((l) => ({
    price: l.price,
    size: isAsk ? l.size : baseForQuote(l.price, l.size),
  }));
}

/** Reconstruct both sides of the book from a decoded slab. */
export function marketViewFromSlab(
  slab: MarketSlab,
  nowSlot: number | bigint,
  nowUnix: number | bigint,
): DropsetMarketView {
  return {
    header: slab.header,
    bids: restingLevels(slab, 'bid', nowSlot, nowUnix),
    asks: restingLevels(slab, 'ask', nowSlot, nowUnix),
  };
}

/** Decode a raw account buffer and reconstruct the book in one step. */
export function decodeDropsetMarketView(
  data: ReadonlyUint8Array,
  nowSlot: number | bigint,
  nowUnix: number | bigint,
): DropsetMarketView {
  return marketViewFromSlab(decodeMarketSlab(data), nowSlot, nowUnix);
}

/**
 * Minimal `getSlot` shape — the slot half of the dual expiry gate. Kept
 * argument-less for structural compatibility with any kit RPC; a caller
 * needing a non-default commitment reads the slot itself and pins it.
 */
export type SlotRpc = { getSlot: (...args: never[]) => { send: () => Promise<bigint> } };

/** Wall-clock now, in the unix seconds level expiry is denominated in. */
export function nowUnix(): number {
  return Math.floor(Date.now() / 1000);
}

/**
 * Fetch a market account and reconstruct its resting book — the reusable
 * live-poll primitive behind the order-book viz. One `getAccountInfo`
 * decodes the whole book.
 *
 * Level expiry is **dual-domain** — each level carries a slot deadline and
 * a wall-clock deadline and rests only inside both — so the filter needs
 * both clocks. The slot comes from `getSlot` unless pinned via
 * `config.nowSlot`; the wall clock defaults to the browser's.
 */
export async function fetchDropsetMarketView(
  rpc: Parameters<typeof fetchEncodedAccount>[0] & SlotRpc,
  address: Address,
  config?: FetchAccountConfig & { nowSlot?: number | bigint; nowUnix?: number | bigint },
): Promise<DropsetMarketView> {
  const account = await fetchEncodedAccount(rpc, address, config);
  assertAccountExists(account);
  const nowSlot = config?.nowSlot ?? (await rpc.getSlot().send());
  return decodeDropsetMarketView(account.data, nowSlot, config?.nowUnix ?? nowUnix());
}
