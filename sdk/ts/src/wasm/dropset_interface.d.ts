/* tslint:disable */
/* eslint-disable */

/**
 * Result of [`simulate_swap`].
 */
export class Quote {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    readonly fee_amount: bigint;
    readonly in_amount: bigint;
    readonly legs: number;
    readonly out_amount: bigint;
    readonly platform_fee_amount: bigint;
}

/**
 * Both sides of the reconstructed resting book, as parallel flat arrays.
 *
 * Each side is two equal-length arrays rather than an array of level
 * objects: wasm-bindgen maps `Vec<u32>` / `Vec<u64>` onto `Uint32Array` /
 * `BigUint64Array`, so a book crosses the boundary as four typed arrays
 * instead of one JS object per level, each of which the caller would have
 * to `free()`. `prices[i]` and `sizes[i]` describe level `i`.
 *
 * `sizes` are **base atoms on both sides** — an ask carries base directly,
 * a bid's matchable quote leg is converted to base at the level price (and
 * saturated to `u64`) by [`resting_levels`](crate::matching::resting_levels)
 * — so the two sides are directly comparable.
 */
export class RestingBook {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Ask prices as raw `Price` bits, best (lowest) first.
     */
    readonly ask_prices: Uint32Array;
    /**
     * Ask depth in base atoms, aligned with [`Self::ask_prices`].
     */
    readonly ask_sizes: BigUint64Array;
    /**
     * Bid prices as raw `Price` bits, best (highest) first.
     */
    readonly bid_prices: Uint32Array;
    /**
     * Bid depth in base atoms, aligned with [`Self::bid_prices`].
     */
    readonly bid_sizes: BigUint64Array;
}

/**
 * `quote / price`, rounded toward zero (saturated to u64).
 */
export function price_base_for_quote(bits: number, quote: bigint): bigint;

/**
 * Decode raw `Price` bits to a number (`0` / `Infinity` for sentinels).
 */
export function price_decode(bits: number): number;

/**
 * Encode a decimal price (e.g. `1.085`) to raw `Price` bits, or `None`
 * (JS `undefined`) if out of range.
 */
export function price_encode(value: number): number | undefined;

/**
 * Whether `bits` is a valid `Price` encoding.
 */
export function price_is_valid(bits: number): boolean;

/**
 * `base * price`, rounded toward zero (saturated to u64).
 */
export function price_quote_for_base(bits: number, base: bigint): bigint;

/**
 * Reconstruct **both sides** of the resting book from a market account's
 * raw data (including the 8-byte discriminator) — the depth view behind an
 * order-book UI, and the same book [`simulate_swap`] fills against.
 *
 * Level expiry is **dual-domain**: `now_slot` is the current slot and
 * `now_unix` the current wall-clock time in unix **seconds**, and a level
 * rests only while it is inside both of its deadlines. Passing one where
 * the other belongs silently resurrects expired levels (or kills live
 * ones).
 *
 * Both sides come from one `MarketView::load`, so a UI polling the book
 * pays a single decode per account fetch. An empty side means either no
 * live levels or a book the engine would reject (a corrupt active list) —
 * a router must not show depth the engine won't fill.
 *
 * Note the side mapping: `SwapSide::Buy` *takes from* the asks, so the ask
 * side is collected with `Buy` and the bid side with `Sell`.
 */
export function resting_book(market_data: Uint8Array, now_slot: number, now_unix: number): RestingBook;

/**
 * Simulate a take against a market account's raw data (including the
 * 8-byte discriminator). `side`: 0 = buy, 1 = sell. `limit_price_bits`:
 * raw `Price` bits (use the per-side no-bound sentinel to disable).
 * Level expiry is **dual-domain**: `now_slot` is the current slot and
 * `now_unix` the current wall-clock time in unix **seconds**, and a
 * level is shown only while it is inside both of its deadlines. Passing
 * one where the other belongs silently resurrects expired levels (or
 * kills live ones). `platform_fee_bps`: the integrator fee
 * the caller will declare on the `swap` instruction — `0` for an
 * unrouted quote. A rate above the market's ceiling yields an all-zero
 * `Quote`, matching the engine's refusal.
 */
export function simulate_swap(market_data: Uint8Array, side: number, amount_in: bigint, limit_price_bits: number, now_slot: number, now_unix: number, platform_fee_bps: number): Quote;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_quote_free: (a: number, b: number) => void;
    readonly __wbg_restingbook_free: (a: number, b: number) => void;
    readonly quote_fee_amount: (a: number) => bigint;
    readonly quote_in_amount: (a: number) => bigint;
    readonly quote_legs: (a: number) => number;
    readonly quote_out_amount: (a: number) => bigint;
    readonly quote_platform_fee_amount: (a: number) => bigint;
    readonly resting_book: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly restingbook_ask_prices: (a: number) => [number, number];
    readonly restingbook_ask_sizes: (a: number) => [number, number];
    readonly restingbook_bid_prices: (a: number) => [number, number];
    readonly restingbook_bid_sizes: (a: number) => [number, number];
    readonly simulate_swap: (a: number, b: number, c: number, d: bigint, e: number, f: number, g: number, h: number) => [number, number, number];
    readonly price_base_for_quote: (a: number, b: bigint) => bigint;
    readonly price_decode: (a: number) => number;
    readonly price_encode: (a: number) => number;
    readonly price_is_valid: (a: number) => number;
    readonly price_quote_for_base: (a: number, b: bigint) => bigint;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
