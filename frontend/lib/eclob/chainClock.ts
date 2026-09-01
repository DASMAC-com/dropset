// The client-side corrections behind the dual-domain expiry gate: the
// wall-clock half, checked against the chain rather than taken on trust from
// the visitor's device, and the slot half's forward margin.
//
// A resting level is live only inside *both* of its deadlines — a slot
// deadline and a wall-clock one — and the engine measures the second against
// cluster time (`Clock.unix_timestamp`), not against whoever is looking at the
// page. Feeding the book mirror a raw `Date.now()` therefore predicts the
// engine's ruling with an unbounded consumer clock: a browser a minute slow
// renders levels the engine has already dropped (the swap then soft-reverts on
// `min_out` — moving no funds, but still spending the network fee and any
// first-time ATA rent), and one a minute fast hides live depth so the book
// reads thin. Tens of seconds of skew is ordinary on consumer machines.
//
// The slot half needs no chain check — the slot *is* a chain read — but it
// needs the same forward nudge for a different reason: it is read at
// `confirmed`, which is already behind head. See {@link gateNowSlot}.
//
// Every poll tick already reads the current slot, so cluster time is one
// `getBlockTime(slot)` away — a skew detector and a fallback for the price of
// one extra read, with no change to the mirror's own API. The shape is a
// ramped "best estimate, chain fallback":
//
//   * within CLOCK_SKEW_TOLERANCE_SECS — the device clock is fine, use it.
//     `getBlockTime` is itself a stake-weighted mean of validator vote
//     timestamps, so it carries its own noise; correcting unconditionally
//     would import that noise into an already-good clock. This is the common
//     path, so most of the time the gate *is* the device clock — vouched for
//     by the chain, not replaced by it.
//   * beyond CLOCK_SKEW_FULL_CORRECTION_SECS — gate with the fully
//     offset-corrected estimate instead. Silent: a skewed clock is not
//     something the visitor can act on.
//   * between the two — phase the correction in proportionally, so the gate
//     is a continuous function of the reading instead of a cliff at the
//     tolerance edge. A step there moved the gate by the *entire* offset
//     magnitude — about 6 s at the edge — in response to a 1 s change in a
//     measurement that carries about a second of its own quantization, so a
//     device parked near the edge alternated between the two paths across
//     ticks, and levels near expiry flickered in and out of the ladder at
//     the poll cadence.
//   * no reading yet (cold start, or a slot the RPC won't time) — the device
//     clock stands.
//
// Ramping rather than dead-banding is deliberate. Subtracting the tolerance
// from the offset and correcting only the excess would be smoother still —
// the gate would never move more than a second per second of measurement
// noise — but it leaves the gate permanently ~5 s behind cluster time at
// *every* skew, including the tens-of-seconds case the module exists for.
// That residue points the expensive way: a gate behind cluster time shows
// levels the engine has already dropped, costing the taker a soft revert —
// no funds moved, the network fee spent — where a gate ahead of it merely
// hides a sliver of depth. The
// ramp instead pays a bounded, temporary version of that error inside the
// band — where the stakes are smallest, because the skew is small — and is
// exact once past it.
//
// The offset lives at module scope and is re-read on an interval rather than
// per tick, so every consumer shares one reading and four poll chains cost one
// request per interval between them. A tick that can't get a reading carries
// the last-known one rather than snapping back to the raw device clock.
//
// The reading is range-checked before it is allowed to become an offset. A
// node answering in milliseconds (~1.7e12 where a unix second is ~1.7e9)
// would otherwise write an offset of roughly 54,000 years — and because the
// ramp applies the whole of a large offset, that poisoned value is
// *guaranteed* to reach the gate, blanking the entire book behind nothing
// louder than the existing `outAmount === 0n` throw.
//
// The check is on the ABSOLUTE reading, never on the derived offset. A bound
// on the offset would reject precisely the devices this module exists to
// correct: a machine with a dead CMOS battery, or a phone set to the wrong
// year, is skewed by far more than any plausible bound, so such a check
// would discard the good chain reading and keep the catastrophic device
// clock. A rejected reading leaves the previous offset in place, exactly
// like a transport error.
//
// That range check is robustness against a malfunctioning node, not a
// security boundary. The same RPC already serves the raw market bytes the
// whole simulation runs on — a strictly stronger lever than the clock — and
// the engine re-derives its own `now_unix` regardless, so this gate can
// never authorize a fill the engine would refuse.

import { type SlotTime, slotTime, type WallTime, wallTime } from "@dropset/sdk";

import {
  CLOCK_PLAUSIBLE_MAX_UNIX,
  CLOCK_PLAUSIBLE_MIN_UNIX,
  CLOCK_RESYNC_INTERVAL_SECS,
  CLOCK_SAFETY_MARGIN_SECS,
  CLOCK_SKEW_FULL_CORRECTION_SECS,
  CLOCK_SKEW_TOLERANCE_SECS,
  SLOT_SAFETY_MARGIN_SLOTS,
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
// Set it from the browser console against a running book. At or beyond
// CLOCK_SKEW_FULL_CORRECTION_SECS the ladder and the quote should be unmoved
// either way, because the offset read off the chain cancels the whole of
// what is set here. *Inside* the ramp band the cancellation is deliberately
// partial — at ±6 only about one second of the six is corrected — so the
// ladder does move, and that is the ramp working rather than a failure. To
// exercise the band edges, probe ±5 / ±6 (the old cliff, where the gate
// should now be identical either side) and ±9 / ±10 (where the ramp's
// steepest remaining step sits). Changing it forces the next call to re-read
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

/**
 * Whether a raw `getBlockTime` answer is worth deriving an offset from.
 *
 * Rejects a non-finite reading and anything outside the plausibility window,
 * which is what catches the millisecond-unit case. Pure, and exported so the
 * boundaries can be pinned directly — the interesting inputs (a millisecond
 * reading, zero, a negative) are ones a healthy node never produces, so there
 * is no way to reach them through {@link syncChainClock} against real RPC.
 */
export const isPlausibleChainSecs = (chainSecs: number): boolean =>
  Number.isFinite(chainSecs) &&
  chainSecs >= CLOCK_PLAUSIBLE_MIN_UNIX &&
  chainSecs <= CLOCK_PLAUSIBLE_MAX_UNIX;

/**
 * How much of a chain-minus-device offset to actually apply to the gate.
 *
 * Zero within CLOCK_SKEW_TOLERANCE_SECS, the whole offset beyond
 * CLOCK_SKEW_FULL_CORRECTION_SECS, and phased in proportionally between —
 * continuous across both edges, so a second of measurement noise moves the
 * gate by at most 3 s rather than the offset's full magnitude (~6 s at the
 * old cliff). Note the ramp is quadratic in the offset, so that worst case
 * sits at the *top* of the band rather than at the tolerance edge, where the
 * step is 1 s: this halves the discontinuity, it does not remove it.
 * Sign-preserving: a device that runs fast gets a negative correction.
 *
 * Rounded to whole seconds to match the integral unix seconds the on-chain
 * fields store. The half-second that rounding can introduce is well inside
 * `getBlockTime`'s own quantization, let alone CLOCK_SAFETY_MARGIN_SECS.
 *
 * Pure, and exported for the same reason as {@link isPlausibleChainSecs}.
 */
export const skewCorrectionSecs = (offsetSecs: number): number => {
  const band = CLOCK_SKEW_FULL_CORRECTION_SECS - CLOCK_SKEW_TOLERANCE_SECS;
  const excess = Math.abs(offsetSecs) - CLOCK_SKEW_TOLERANCE_SECS;
  // Returning early rather than clamping the weight at zero: scaling a
  // negative offset by a zero weight yields -0, which is arithmetically
  // harmless but a wart to hand back from something documented as returning
  // no correction. The `band` half of the guard covers the two constants
  // being reordered — a non-positive band would otherwise drive the weight
  // negative and apply the correction with the wrong *sign*, pushing the
  // gate away from cluster time instead of toward it.
  if (excess <= 0 || band <= 0) return 0;
  return Math.round(offsetSecs * Math.min(excess / band, 1));
};

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
    // Range-check the reading itself, not the offset it would produce — see
    // the header. A rejection leaves the previous offset standing, the same
    // as a slot the node won't time above, or a transport error below.
    const chain = Number(chainSecs);
    if (!isPlausibleChainSecs(chain)) return;
    offsetSecs = chain - device;
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
 * close enough to the cluster's, the offset-corrected estimate once it is far
 * enough away, a proportional blend of the two in between (see
 * {@link skewCorrectionSecs}), and in every case nudged forward by a small
 * safety margin so a level in its last moments is dropped here before the
 * engine drops it mid-swap.
 *
 * Reads the offset cached by {@link syncChainClock}; call that first each tick
 * so the reading stays current. Cheap and synchronous, so it can be called at
 * the point of use rather than threaded through.
 *
 * Returns a `WallTime`, the SDK's wall clock-domain brand: this is the
 * frontend's wall-domain source of truth, and the slot half of the dual
 * expiry gate is a different type, so the two can no longer be passed to
 * each other's parameter.
 */
export const gateNowUnix = (): WallTime => {
  const device = deviceNowUnix();
  const corrected =
    offsetSecs === null ? device : device + skewCorrectionSecs(offsetSecs);
  return wallTime(corrected + CLOCK_SAFETY_MARGIN_SECS);
};

/**
 * The slot to gate resting levels against, given the `confirmed` slot the
 * caller has already read: that slot nudged forward by
 * {@link SLOT_SAFETY_MARGIN_SLOTS}.
 *
 * The slot half of the gate needs no chain check — it *is* a chain read — but
 * it does need the same forward margin {@link gateNowUnix} applies, and for a
 * sharper reason. A `confirmed` slot is behind head when it is read and
 * further behind by the time the transaction lands, so passing it raw reports
 * levels as more alive than the engine will find them. That is precisely the
 * over-showing the wall margin exists to prevent, arriving through the other
 * conjunct.
 *
 * Takes the slot rather than reading one so the gate and the account fetch
 * stay pinned to the same commitment (see `useOrderBook`), and so a caller
 * pays one `getSlot` for both this and {@link syncChainClock}.
 *
 * Pass the **raw** slot to {@link syncChainClock} and the margined one only
 * here: `getBlockTime` is asking for the production time of a block that
 * exists, and a slot nudged past head has none.
 *
 * Returns a `SlotTime`, the SDK's slot-domain brand — the counterpart to
 * `gateNowUnix`'s `WallTime`, so the two halves of the gate cannot be passed
 * to each other's parameter.
 */
export const gateNowSlot = (confirmedSlot: bigint | number): SlotTime =>
  slotTime(Number(confirmedSlot) + SLOT_SAFETY_MARGIN_SLOTS);
