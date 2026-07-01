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
 * Simulate a take against a market account's raw data (including the
 * 8-byte discriminator). `side`: 0 = buy, 1 = sell. `limit_price_bits`:
 * raw `Price` bits (use the per-side no-bound sentinel to disable).
 */
export function simulate_swap(market_data: Uint8Array, side: number, amount_in: bigint, limit_price_bits: number, current_slot: number): Quote;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_quote_free: (a: number, b: number) => void;
    readonly quote_fee_amount: (a: number) => bigint;
    readonly quote_in_amount: (a: number) => bigint;
    readonly quote_legs: (a: number) => number;
    readonly quote_out_amount: (a: number) => bigint;
    readonly simulate_swap: (a: number, b: number, c: number, d: bigint, e: number, f: number) => [number, number, number];
    readonly price_base_for_quote: (a: number, b: bigint) => bigint;
    readonly price_decode: (a: number) => number;
    readonly price_encode: (a: number) => number;
    readonly price_is_valid: (a: number) => number;
    readonly price_quote_for_base: (a: number, b: bigint) => bigint;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
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
