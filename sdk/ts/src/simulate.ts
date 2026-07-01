// cspell:word turbopack
/**
 * Off-chain swap simulation — the eCLOB quoter.
 *
 * Thin, ergonomic wrapper over the WASM `simulate_swap` binding compiled
 * from the `dropset-interface` Rust crate (`make wasm` → `./wasm`). The
 * binding reconstructs the order book from a market account's raw bytes and
 * runs the *exact* on-chain matching math (shared `dropset-math-core`,
 * pinned by the conformance vectors), so a client-side quote equals the
 * on-chain fill — no hand-mirrored second implementation to drift.
 *
 * WASM must be instantiated once before the first {@link simulateSwap} call;
 * {@link initSimulator} does that and is idempotent. Pass the no-bound
 * `Price` sentinels from {@link ./price} (`PRICE_INFINITY` for a market buy,
 * `PRICE_ZERO` for a market sell) as `limitPriceBits` to quote a market take.
 */

import initWasm, {
  type InitInput,
  simulate_swap as wasmSimulateSwap,
} from './wasm/dropset_interface';
import type { PriceBits } from './price';

/** Take direction: `buy` spends quote for base, `sell` spends base for quote. */
export type SwapSide = 'buy' | 'sell';

/** The on-chain `SwapSide` discriminant the WASM binding expects. */
const SIDE_CODE: Record<SwapSide, number> = { buy: 0, sell: 1 };

/** A simulated take result — the plain-object mirror of the WASM `Quote`. */
export interface SimulatedQuote {
  /** Input atoms actually consumed (≤ requested when the book is thin). */
  inAmount: bigint;
  /** Output atoms produced. */
  outAmount: bigint;
  /** Taker fee atoms included in the fill. */
  feeAmount: bigint;
  /** Number of resting levels the take crossed. */
  legs: number;
}

let initPromise: Promise<void> | null = null;

/**
 * Instantiate the WASM simulator once. Idempotent — repeated calls await the
 * same instantiation. With no argument the binding resolves its `.wasm`
 * asset relative to the module (via `import.meta.url`), which bundlers
 * (turbopack / webpack) emit automatically; pass an {@link InitInput}
 * (URL, `Response`, bytes, or a compiled `Module`) to override — e.g. in a
 * Node test where there is no asset pipeline.
 */
export function initSimulator(input?: InitInput): Promise<void> {
  if (initPromise === null) {
    initPromise = initWasm(
      input === undefined ? undefined : { module_or_path: input },
    ).then(() => undefined);
  }
  return initPromise;
}

/**
 * Simulate a take against a market account's raw bytes (including the 8-byte
 * account discriminator — pass the account data verbatim). `limitPriceBits`
 * is raw {@link PriceBits}; use `PRICE_INFINITY` (buy) / `PRICE_ZERO` (sell)
 * for an unbounded market take. `currentSlot` scopes level expiry.
 *
 * {@link initSimulator} must have resolved first, else the binding throws.
 */
export function simulateSwap(
  marketData: Uint8Array,
  side: SwapSide,
  amountIn: bigint,
  limitPriceBits: PriceBits,
  currentSlot: number,
): SimulatedQuote {
  const q = wasmSimulateSwap(
    marketData,
    SIDE_CODE[side],
    amountIn,
    limitPriceBits,
    currentSlot,
  );
  try {
    return {
      inAmount: q.in_amount,
      outAmount: q.out_amount,
      feeAmount: q.fee_amount,
      legs: q.legs,
    };
  } finally {
    // The WASM `Quote` owns linear-memory; release it once marshalled out.
    q.free();
  }
}
