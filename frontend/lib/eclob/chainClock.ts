// The wall-clock half of the dual-domain expiry gate, sourced from the chain
// rather than from the visitor's device.
//
// A resting level is live only inside *both* of its deadlines — a slot
// deadline and a wall-clock one — and the engine measures the second against
// cluster time (`Clock.unix_timestamp`), not against whoever is looking at the
// page. Feeding the book mirror a raw `Date.now()` therefore predicts the
// engine's ruling with an unbounded consumer clock: a browser a minute slow
// renders levels the engine has already dropped (the swap then soft-reverts on
// `min_out`, and the taker still pays fees and any ATA rent), and one a minute
// fast hides live depth so the book reads thin. Tens of seconds of skew is
// ordinary on consumer machines.
//
// Every poll tick already reads the current slot, so cluster time is one
// `getBlockTime(slot)` away — a skew detector and a fallback for the price of
// one extra read, with no change to the mirror's own API. The shape is "best
// estimate, chain fallback":
//
//   * within CLOCK_SKEW_TOLERANCE_SECS — the device clock is fine, use it.
//     `getBlockTime` is itself a stake-weighted mean of validator vote
//     timestamps, so it carries its own noise; correcting unconditionally
//     would import that noise into an already-good clock.
//   * beyond it — gate with the offset-corrected estimate instead. Silent:
//     a skewed clock is not something the visitor can act on.
//   * no reading yet (cold start, or a slot the RPC won't time) — the device
//     clock stands.
//
// The offset lives at module scope, so every consumer benefits from whichever
// one refreshed it last, and a tick that can't get a reading carries the
// last-known one rather than snapping back to the raw device clock.

import {
  CLOCK_SAFETY_MARGIN_SECS,
  CLOCK_SKEW_TOLERANCE_SECS,
} from "../data/timings";

// Minimal structural shape of the one RPC method this needs. Declared here
// rather than imported so the module stays independent of which client the
// caller holds — the same reason the SDK types its own `SlotRpc` this way.
// `getBlockTime` answers with the block's production time in unix seconds, or
// null for a slot the node has no block for.
export type BlockTimeRpc = {
  getBlockTime: (...args: never[]) => { send: () => Promise<bigint | null> };
};

// Chain-minus-device offset in whole seconds, or null before the first
// successful reading. Module scope on purpose: the order book, the eCLOB
// quote, the router quote and the swap builder all gate against the same
// cluster, so a reading taken by any of them is good for all of them.
let offsetSecs: number | null = null;

// Artificial device-clock skew in seconds, for exercising the correction
// without physically mis-setting the machine's clock:
//
//   __dropsetClockSkewSecs = -60   // pretend this browser runs a minute slow
//   __dropsetClockSkewSecs = 60    // …a minute fast
//   delete __dropsetClockSkewSecs  // back to the real clock
//
// Set it from the browser console against a running book: the ladder and the
// quote should be unmoved either way, because the offset read off the chain
// cancels whatever is set here. Compiled out of a production build, and inert
// until something sets it.
const debugSkewSecs = (): number => {
  if (process.env.NODE_ENV === "production") return 0;
  const skew = (globalThis as { __dropsetClockSkewSecs?: unknown })
    .__dropsetClockSkewSecs;
  return typeof skew === "number" && Number.isFinite(skew) ? skew : 0;
};

// The device clock, in the unix seconds the on-chain fields store.
const deviceNowUnix = (): number =>
  Math.floor(Date.now() / 1_000) + debugSkewSecs();

/**
 * Refresh the cached chain-minus-device offset from a slot the caller has
 * already read. Best-effort: a slot the node won't time (skipped, or aged out
 * of its ledger) and any transport error both leave the previous offset in
 * place, since clock skew drifts far more slowly than a poll tick.
 *
 * The reading runs a beat behind true cluster time — it is the production time
 * of an already-confirmed block, read across a round-trip — so the offset
 * carries a bias of order a second, biased *backwards*. That is well inside
 * CLOCK_SKEW_TOLERANCE_SECS, so it can't by itself push a healthy clock onto
 * the corrected path, and on the corrected path the forward safety margin
 * absorbs it.
 */
export const syncChainClock = async (
  rpc: BlockTimeRpc,
  slot: bigint | number,
): Promise<void> => {
  try {
    const chainSecs = await rpc.getBlockTime(slot as never).send();
    if (chainSecs === null) return;
    offsetSecs = Number(chainSecs) - deviceNowUnix();
  } catch {
    // Leave `offsetSecs` alone — a stale offset beats no correction, and the
    // caller's own error handling owns whatever else this tick was doing.
  }
};

/**
 * The unix second to gate resting levels against: the device clock when it is
 * close enough to the cluster's, the offset-corrected estimate when it isn't,
 * and either way nudged forward by a small safety margin so a level in its
 * last moments is dropped here before the engine drops it mid-swap.
 *
 * Reads the offset cached by {@link syncChainClock}; call that first each tick
 * if a fresh reading matters. Cheap and synchronous, so it can be called at
 * the point of use rather than threaded through.
 */
export const gateNowUnix = (): number => {
  const device = deviceNowUnix();
  const corrected =
    offsetSecs !== null && Math.abs(offsetSecs) > CLOCK_SKEW_TOLERANCE_SECS
      ? device + offsetSecs
      : device;
  return corrected + CLOCK_SAFETY_MARGIN_SECS;
};

/**
 * The cached offset in seconds, or null before any reading landed. Exported
 * for the skew test hook and for diagnostics; nothing in the render path needs
 * it, since {@link gateNowUnix} already applies it.
 */
export const chainClockOffsetSecs = (): number | null => offsetSecs;
