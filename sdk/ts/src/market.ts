/**
 * On-chain market-account reader: decode the market header and read the
 * resting order book.
 *
 * The reusable on-chain read primitive behind the order-book viz. A market
 * is a single account, so a live poll is one `getAccountInfo`:
 * {@link fetchDropsetMarketView} fetches it (plus the current slot, for
 * expiry filtering) and returns `{ header, bids, asks }`.
 *
 * The account is stored as `Slab<MarketHeader, Vault>`: an 8-byte Anchor
 * discriminator, a fixed `MarketHeader`, a `u32` slab length, alignment
 * padding, then a tail of fixed-size `Vault` sectors. The two halves are
 * read by the two different mechanisms that can each be pinned to the
 * program:
 *
 * - The **header** is IDL-described, so it is decoded by the Codama-generated
 *   `MarketHeader` codec, which CI regenerates and diffs against the IDL.
 * - The **`Vault` slab** is *not* IDL-describable — it is a hand-managed,
 *   `bytemuck`-cast arena of fixed-stride sectors threaded by an intrusive
 *   doubly-linked list, which Anchor's IDL vocabulary has no way to express.
 *   So it is decoded by the engine itself, through the WASM binding compiled
 *   from `dropset-interface` (`layout.rs` + `matching.rs`).
 *
 * That second point is the whole design. This module used to hand-mirror the
 * slab byte offsets and re-implement `collect_side_levels` / `level_state` /
 * `resting_levels` in TypeScript, which meant the on-chain layout was
 * restated in a second language with nothing mechanically holding the two
 * together — and it silently drifted when the `Vault` layout grew. Reading
 * through {@link ./simulate | the shared WASM module} removes the drift
 * surface structurally rather than testing for it: there is only one
 * implementation of the book, and it is the one the chain runs.
 *
 * The module is therefore WASM-gated. {@link initSimulator} instantiates the
 * shared binding (it backs both this reader and the swap simulator) and must
 * have resolved before {@link decodeDropsetMarketView};
 * {@link fetchDropsetMarketView} awaits it for you, so a caller polling the
 * book never has to think about it.
 */

import {
  assertAccountExists,
  fetchEncodedAccount,
  type Address,
  type FetchAccountConfig,
  type ReadonlyUint8Array,
} from '@solana/kit';

import { type SlotTime, slotTime, type WallTime, wallTime } from './clock';
import { getMarketHeaderDecoder, getMarketHeaderSize, type MarketHeader } from './generated';
import { initSimulator } from './simulate';
import { resting_book as wasmRestingBook, type RestingBook } from './wasm/dropset_interface';
import type { PriceBits } from './price';

/** A resting level: an absolute `price` and its depth in **base atoms**. */
export type BookLevel = { price: PriceBits; size: bigint };

/** The reconstructed book: the header and both sides, best price first. */
export type DropsetMarketView = { header: MarketHeader; bids: BookLevel[]; asks: BookLevel[] };

/** Raised when the raw account bytes can't be decoded as a market slab. */
export class MarketLayoutError extends Error {}

/**
 * Zip one side's parallel `(prices, sizes)` arrays into {@link BookLevel}s.
 * The binding hands each side across as two typed arrays rather than an
 * array of objects, so the boundary allocates nothing per level.
 */
function zipSide(prices: Uint32Array, sizes: BigUint64Array): BookLevel[] {
  const out: BookLevel[] = new Array(prices.length);
  for (let i = 0; i < prices.length; i++) {
    out[i] = { price: prices[i]!, size: sizes[i]! };
  }
  return out;
}

/**
 * Decode a market account's full data buffer (including the 8-byte
 * discriminator) into its header and both sides of the resting book.
 *
 * The book comes from the engine's own reconstruction via the WASM binding,
 * so it is the book the on-chain matcher would fill against at
 * `(nowSlot, nowUnix)` — an empty side means either no live levels or a book
 * the engine would reject, since a router must not show depth that won't
 * fill. Level expiry is **dual-domain**: a level rests only while it is
 * inside both its slot deadline and its wall-clock deadline, so passing one
 * clock where the other belongs silently resurrects expired levels — the
 * two are therefore domain-branded ({@link SlotTime} / {@link WallTime},
 * see `./clock`), which makes that swap a type error rather than a silent
 * mis-filter. They are unwrapped to bare numbers only on the line that
 * crosses into WASM, which is the one place the distinction cannot travel.
 *
 * **The `bigint` arm is an unguarded residual, deliberately kept.** It
 * exists because `rpc.getSlot()` returns a `bigint` and callers pass it
 * straight through, but a `bigint` is assignable to *both* parameters —
 * so `decodeDropsetMarketView(data, 1_700_000_000n, 57n)` still
 * type-checks transposed. The brands guard branded values; they cannot
 * guard a caller that opts out of them. Prefer handing these
 * {@link slotTime} / {@link wallTime} values, which is what
 * {@link fetchDropsetMarketView} does for its own default.
 *
 * {@link initSimulator} must have resolved first, else the binding throws;
 * {@link fetchDropsetMarketView} handles that. Throws
 * {@link MarketLayoutError} when the bytes aren't a well-formed market slab.
 */
export function decodeDropsetMarketView(
  data: ReadonlyUint8Array,
  nowSlot: SlotTime | bigint,
  nowUnix: WallTime | bigint,
): DropsetMarketView {
  // `getMarketHeaderSize()` is the discriminator + header length. Guarded
  // here so a short buffer reports as a layout error rather than surfacing
  // from inside the generated codec.
  const lenAt = getMarketHeaderSize();
  if (data.length < lenAt + 4) {
    throw new MarketLayoutError('account buffer too small for header + slab length');
  }
  const header = getMarketHeaderDecoder().decode(data.subarray(0, lenAt));

  let book: RestingBook;
  try {
    // wasm-bindgen copies the slice into linear memory, so handing it a
    // readonly view is safe.
    book = wasmRestingBook(data as Uint8Array, Number(nowSlot), Number(nowUnix));
  } catch (e) {
    throw new MarketLayoutError(e instanceof Error ? e.message : String(e));
  }
  try {
    return {
      header,
      bids: zipSide(book.bid_prices, book.bid_sizes),
      asks: zipSide(book.ask_prices, book.ask_sizes),
    };
  } finally {
    // The WASM `RestingBook` owns linear memory; the getters above copy out
    // of it, so it can be released as soon as both sides are marshalled.
    book.free();
  }
}

/**
 * Minimal `getSlot` shape — the slot half of the dual expiry gate. Kept
 * argument-less for structural compatibility with any kit RPC; a caller
 * needing a non-default commitment reads the slot itself and pins it.
 */
export type SlotRpc = { getSlot: (...args: never[]) => { send: () => Promise<bigint> } };

/**
 * Wall-clock now, in the unix seconds level expiry is denominated in.
 *
 * Read straight off the device clock, so it is only sound for a caller that
 * can bound its own: a host with disciplined time (the bots, the TUI). A
 * browser cannot, and must gate against a chain-read time instead — which is
 * why no entry point here reaches for this as a default.
 *
 * Domain-branded, so it cannot be handed to the slot half of the dual gate —
 * the slot "now" comes from {@link SlotRpc}, and the two are different types.
 */
export function nowUnix(): WallTime {
  return wallTime(Math.floor(Date.now() / 1000));
}

/**
 * Fetch a market account and reconstruct its resting book — the reusable
 * live-poll primitive behind the order-book viz. One `getAccountInfo`
 * decodes the whole book.
 *
 * Level expiry is **dual-domain** — each level carries a slot deadline and
 * a wall-clock deadline and rests only inside both — so the filter needs
 * both clocks. The two are not symmetric. The slot is a chain read, so it
 * defaults to `getSlot` unless pinned via `config.nowSlot`. The wall clock
 * has no such fallback and is **required**: the engine judges that deadline
 * against cluster time, so a caller that cannot bound its own clock must
 * gate against a chain-read time rather than a device one.
 *
 * Instantiates the shared WASM binding on first use (idempotent, and the
 * same module the swap simulator uses), so callers need no separate init
 * step.
 */
export async function fetchDropsetMarketView(
  rpc: Parameters<typeof fetchEncodedAccount>[0] & SlotRpc,
  address: Address,
  config: FetchAccountConfig & { nowSlot?: SlotTime | bigint; nowUnix: WallTime | bigint },
): Promise<DropsetMarketView> {
  // The account fetch and the WASM instantiation are independent, so overlap
  // them; `initSimulator` is idempotent, so a polling caller pays the
  // instantiation only once.
  const [account] = await Promise.all([
    fetchEncodedAccount(rpc, address, config),
    initSimulator(),
  ]);
  assertAccountExists(account);
  // `getSlot` hands back a bare bigint, so this is the slot domain's
  // boundary on the fetch path — brand it here rather than letting an
  // untagged number reach the gate. `nowUnix` takes no such default: it is
  // a required argument precisely so a caller that cannot bound its own
  // clock has to supply a chain-read one.
  const nowSlot = config.nowSlot ?? slotTime(Number(await rpc.getSlot().send()));
  return decodeDropsetMarketView(account.data, nowSlot, config.nowUnix);
}
