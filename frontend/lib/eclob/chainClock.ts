// The wall-clock half of the dual-domain expiry gate, checked against the
// chain rather than taken on trust from the visitor's device.
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
//     would import that noise into an already-good clock. This is the common
//     path, so most of the time the gate *is* the device clock — vouched for
//     by the chain, not replaced by it.
//   * beyond it — gate with the offset-corrected estimate instead. Silent:
//     a skewed clock is not something the visitor can act on.
//   * no reading yet (cold start, or a slot the RPC won't time) — the device
//     clock stands.
//
// The offset lives at module scope and is re-read on an interval rather than
// per tick, so every consumer shares one reading and four poll chains cost one
// request per interval between them. A tick that can't get a reading carries
// the last-known one rather than snapping back to the raw device clock.
//
// Deliberately NOT done here: hysteresis around the tolerance edge, and a
// plausibility floor on the reading itself. Both are real (a device parked at
// the boundary can flap between the two paths, and a node answering in
// milliseconds would write a nonsense offset), and both are tracked as
// follow-ups rather than folded into the ratified shape.

import {
  CLOCK_RESYNC_INTERVAL_SECS,
  CLOCK_SAFETY_MARGIN_SECS,
  CLOCK_SKEW_TOLERANCE_SECS,
} from "../data/timings";

/**
 * Minimal structural shape of the one RPC method this needs. Declared here
 * rather than imported so the module stays independent of which client the
 * caller holds — the same reason the SDK types its own `SlotRpc` this way.
 * `getBlockTime` answers with the block's production time in unix seconds, or
 * null for a slot the node has no block for.
 */
export type BlockTimeRpc = {
  getBlockTime: (...args: never[]) => { send: () => Promise<bigint | null> };
};

// Chain-minus-device offset in whole seconds, or null before the first
// successful reading. Module scope on purpose: the order book, the eCLOB
// quote, the router quote and the swap builder all gate against the same
// cluster, so a reading taken by any of them is good for all of them.
let offsetSecs: number | null = null;

// Raw device time of the last sync *attempt*, and the debug skew in force at
// it. Attempt rather than success on purpose: a node that rejects or
// rate-limits `getBlockTime` must back off like any other, not retry on every
// tick behind the silent `catch` below.
let lastSyncAt: number | null = null;
let lastSyncSkewSecs = 0;

// Dedupes concurrent syncs. Three hooks poll on independent timers, so without
// this their calls overlap and the slowest response — derived from the oldest
// slot — lands last and wins.
let inFlight: Promise<void> | null = null;

// Artificial device-clock skew in seconds, for exercising the correction
// without physically mis-setting the machine's clock:
//
//   __dropsetClockSkewSecs = -60   // pretend this browser runs a minute slow
//   __dropsetClockSkewSecs = 60    // …a minute fast
//   delete __dropsetClockSkewSecs  // back to the real clock
//
// Set it from the browser console against a running book: the ladder and the
// quote should be unmoved either way, because the offset read off the chain
// cancels whatever is set here. Changing it forces the next call to re-read
// rather than wait out the resync interval, so the effect is visible on the
// following tick. Compiled out of a production build, and inert until
// something sets it.
const debugSkewSecs = (): number => {
  if (process.env.NODE_ENV === "production") return 0;
  const skew = (globalThis as { __dropsetClockSkewSecs?: unknown })
    .__dropsetClockSkewSecs;
  return typeof skew === "number" && Number.isFinite(skew) ? skew : 0;
};

// The real device clock, in the unix seconds the on-chain fields store. Used
// to age the cached reading, so the debug skew can't shift the resync clock.
const rawDeviceNowUnix = (): number => Math.floor(Date.now() / 1_000);

// The device clock as the gate sees it — the real one plus any debug skew.
const deviceNowUnix = (): number => rawDeviceNowUnix() + debugSkewSecs();

const readChainClock = async (
  rpc: BlockTimeRpc,
  slot: bigint | number,
): Promise<void> => {
  // Sample the device clock BEFORE the request. The value that comes back
  // describes a block produced at or before the send, so pairing it with a
  // device time read *after* the round-trip would fold the whole request
  // latency into the offset — and that latency is unbounded on a congested
  // endpoint. Left that way it inverts the fix: a slow enough response drives
  // the offset past the tolerance on a perfectly-synced device and then drags
  // its gate backwards, behind cluster time, which is precisely the
  // over-showing this module exists to prevent.
  const device = deviceNowUnix();
  try {
    const chainSecs = await rpc.getBlockTime(slot as never).send();
    if (chainSecs === null) return;
    offsetSecs = Number(chainSecs) - device;
  } catch {
    // Leave `offsetSecs` alone — a stale offset beats no correction, and the
    // caller's own error handling owns whatever else this tick was doing.
  }
};

/**
 * Refresh the cached chain-minus-device offset from a slot the caller has
 * already read. Safe and cheap to call on every tick: a reading is reused for
 * CLOCK_RESYNC_INTERVAL_SECS, and concurrent callers share one request.
 *
 * Best-effort. A transport error, a rate-limit rejection, or a slot the node
 * declines to time all leave the previous offset in place, since clock skew
 * drifts far more slowly than the interval. The failure is silent by design;
 * the cost of it is that the gate quietly falls back to the device clock,
 * which is the behavior that predates this module rather than a regression.
 *
 * The reading is still a beat behind true cluster time — it is the production
 * time of an already-confirmed block — so the offset carries a backwards bias
 * of roughly the block age. Sampling the device clock before the send keeps
 * the request latency out of that bias; what remains is irreducible, since
 * nothing knows cluster time more recently than the last block.
 */
export const syncChainClock = async (
  rpc: BlockTimeRpc,
  slot: bigint | number,
): Promise<void> => {
  const skew = debugSkewSecs();
  const fresh =
    lastSyncAt !== null &&
    skew === lastSyncSkewSecs &&
    rawDeviceNowUnix() - lastSyncAt < CLOCK_RESYNC_INTERVAL_SECS;
  if (fresh) return;
  if (inFlight) return inFlight;

  lastSyncAt = rawDeviceNowUnix();
  lastSyncSkewSecs = skew;
  inFlight = readChainClock(rpc, slot).finally(() => {
    inFlight = null;
  });
  return inFlight;
};

/**
 * The unix second to gate resting levels against: the device clock when it is
 * close enough to the cluster's, the offset-corrected estimate when it isn't,
 * and either way nudged forward by a small safety margin so a level in its
 * last moments is dropped here before the engine drops it mid-swap.
 *
 * Reads the offset cached by {@link syncChainClock}; call that first each tick
 * so the reading stays current. Cheap and synchronous, so it can be called at
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
