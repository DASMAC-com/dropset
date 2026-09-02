import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  CLOCK_PLAUSIBLE_MAX_UNIX,
  CLOCK_PLAUSIBLE_MIN_UNIX,
  CLOCK_SAFETY_MARGIN_SECS,
  CLOCK_SKEW_FULL_CORRECTION_SECS,
  CLOCK_SKEW_TOLERANCE_SECS,
  SLOT_SAFETY_MARGIN_SLOTS,
} from "../data/timings";
import { isPlausibleChainSecs, skewCorrectionSecs } from "./chainClock";

describe("skew correction ramp", () => {
  it("ignores an offset inside the tolerance", () => {
    for (const offset of [0, 1, 3, 4, 5, -1, -3, -4, -5]) {
      expect(skewCorrectionSecs(offset)).toBe(0);
    }
  });

  it("applies a large offset in full", () => {
    for (const offset of [10, 11, 60, 3_600, -10, -11, -60, -3_600]) {
      expect(skewCorrectionSecs(offset)).toBe(offset);
    }
  });

  it("phases the correction in across the band", () => {
    // offset * (|offset| - 5) / 5, rounded.
    expect(skewCorrectionSecs(6)).toBe(1); // 1.2
    expect(skewCorrectionSecs(7)).toBe(3); // 2.8
    expect(skewCorrectionSecs(8)).toBe(5); // 4.8
    expect(skewCorrectionSecs(9)).toBe(7); // 7.2
  });

  it("preserves the sign, so a fast device is corrected backwards", () => {
    expect(skewCorrectionSecs(-6)).toBe(-1);
    expect(skewCorrectionSecs(-7)).toBe(-3);
    expect(skewCorrectionSecs(-8)).toBe(-5);
    expect(skewCorrectionSecs(-9)).toBe(-7);
  });

  // The regression this shape exists for. `getBlockTime` carries about a
  // second of its own quantization, so a device parked near an edge measures
  // an offset that flickers between adjacent integers across ticks. Under the
  // old all-or-nothing rule the 5 → 6 crossing moved the gate by the entire
  // offset — 6 s for a 1 s change in the measurement — and levels expiring in
  // the crossed window flickered in and out of the ladder at the poll
  // cadence.
  //
  // The bound below is 3 rather than 1 because the ramp is quadratic: its
  // steepest step is the ±9 ↔ ±10 crossing, not the tolerance edge. So this
  // halves the discontinuity rather than removing it — a device parked near
  // ±9.5 can still move the gate 3 s across ticks. The bound is tight (3 is
  // exactly attained), so any regression in the shape fails here.
  it("never moves the gate far for a one-second change in the reading", () => {
    for (let offset = -120; offset < 120; offset += 1) {
      const step = Math.abs(
        skewCorrectionSecs(offset + 1) - skewCorrectionSecs(offset),
      );
      expect(step).toBeLessThanOrEqual(3);
    }
  });

  it("is continuous across the old cliff at the tolerance edge", () => {
    const step = Math.abs(
      skewCorrectionSecs(CLOCK_SKEW_TOLERANCE_SECS + 1) -
        skewCorrectionSecs(CLOCK_SKEW_TOLERANCE_SECS),
    );
    expect(step).toBe(1);
  });

  it("is continuous where the correction reaches full strength", () => {
    expect(skewCorrectionSecs(CLOCK_SKEW_FULL_CORRECTION_SECS)).toBe(
      CLOCK_SKEW_FULL_CORRECTION_SECS,
    );
    expect(skewCorrectionSecs(CLOCK_SKEW_FULL_CORRECTION_SECS + 1)).toBe(
      CLOCK_SKEW_FULL_CORRECTION_SECS + 1,
    );
  });

  // Both sweeps above assert magnitudes only, which a non-monotone variant of
  // the ramp would satisfy. Pin the shape itself: more skew must never earn
  // less correction.
  it("is monotone in the offset", () => {
    for (let offset = 0; offset < 120; offset += 1) {
      expect(skewCorrectionSecs(offset + 1)).toBeGreaterThanOrEqual(
        skewCorrectionSecs(offset),
      );
      expect(skewCorrectionSecs(-offset - 1)).toBeLessThanOrEqual(
        skewCorrectionSecs(-offset),
      );
    }
  });

  // Correcting by less than the offset leaves the gate behind cluster time,
  // which is the direction that costs a taker a soft revert. Inside the band
  // that shortfall is accepted; it must never exceed the tolerance, and must
  // vanish once the correction is at full strength.
  it("keeps the shortfall bounded by the tolerance, and zero past the band", () => {
    for (let offset = -120; offset <= 120; offset += 1) {
      const shortfall = Math.abs(offset - skewCorrectionSecs(offset));
      expect(shortfall).toBeLessThanOrEqual(CLOCK_SKEW_TOLERANCE_SECS);
      if (Math.abs(offset) >= CLOCK_SKEW_FULL_CORRECTION_SECS) {
        expect(shortfall).toBe(0);
      }
    }
  });
});

describe("chain reading plausibility", () => {
  it("accepts a current-era reading", () => {
    expect(isPlausibleChainSecs(1_780_000_000)).toBe(true);
  });

  it("accepts the window boundaries", () => {
    expect(isPlausibleChainSecs(CLOCK_PLAUSIBLE_MIN_UNIX)).toBe(true);
    expect(isPlausibleChainSecs(CLOCK_PLAUSIBLE_MAX_UNIX)).toBe(true);
  });

  // The motivating case: a node answering in milliseconds. Note this is ABOVE
  // the window, not below it — a floor alone would let it straight through.
  it("rejects a millisecond-valued reading", () => {
    expect(isPlausibleChainSecs(1_780_000_000_000)).toBe(false);
    expect(isPlausibleChainSecs(CLOCK_PLAUSIBLE_MAX_UNIX + 1)).toBe(false);
  });

  it("rejects a reading below the floor", () => {
    expect(isPlausibleChainSecs(CLOCK_PLAUSIBLE_MIN_UNIX - 1)).toBe(false);
    expect(isPlausibleChainSecs(0)).toBe(false);
    expect(isPlausibleChainSecs(-1)).toBe(false);
  });

  it("rejects a non-finite reading", () => {
    expect(isPlausibleChainSecs(Number.NaN)).toBe(false);
    expect(isPlausibleChainSecs(Number.POSITIVE_INFINITY)).toBe(false);
    expect(isPlausibleChainSecs(Number.NEGATIVE_INFINITY)).toBe(false);
  });
});

// The offset lives at module scope, so these re-import the module per test to
// start from a cold cache. Fake timers pin the device clock: the resync
// interval gates on it, so a second sync in the same test has to advance it
// past CLOCK_RESYNC_INTERVAL_SECS or it is skipped as still fresh.
describe("the gate against a cached chain offset", () => {
  const DEVICE_SECS = 1_780_000_000;
  const rpcReturning = (secs: bigint | null) => ({
    getBlockTime: () => ({ send: async () => secs }),
  });
  const rpcThrowing = () => ({
    getBlockTime: () => ({
      send: async (): Promise<bigint | null> => {
        throw new Error("transport");
      },
    }),
  });

  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
    vi.setSystemTime(DEVICE_SECS * 1_000);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  // The module header calls this the common path: most of the time the gate
  // *is* the device clock, vouched for by the chain rather than replaced by
  // it. Nothing else here pins it — the pure tests never touch the cached
  // offset, so this is the case a refactor of the gate's ternary would break
  // silently.
  it("leaves the gate on the device clock for an in-tolerance offset", async () => {
    const { gateNowUnix, syncChainClock } = await import("./chainClock");

    await syncChainClock(rpcReturning(BigInt(DEVICE_SECS + 3)), 1n);
    expect(gateNowUnix()).toBe(DEVICE_SECS + CLOCK_SAFETY_MARGIN_SECS);
  });

  // And the ramp reaching the gate, rather than only the pure function: a +7
  // offset earns 3 s of correction, not 0 and not the full 7.
  it("applies a partial correction for a mid-band offset", async () => {
    const { gateNowUnix, syncChainClock } = await import("./chainClock");

    await syncChainClock(rpcReturning(BigInt(DEVICE_SECS + 7)), 1n);
    expect(gateNowUnix()).toBe(DEVICE_SECS + 3 + CLOCK_SAFETY_MARGIN_SECS);
  });

  it("keeps a good offset when a millisecond reading follows it", async () => {
    const { gateNowUnix, syncChainClock } = await import("./chainClock");

    // A device a minute slow, corrected in full.
    await syncChainClock(rpcReturning(BigInt(DEVICE_SECS + 60)), 1n);
    expect(gateNowUnix()).toBe(DEVICE_SECS + 60 + CLOCK_SAFETY_MARGIN_SECS);

    // Past the resync interval, the same node starts answering in ms.
    const later = DEVICE_SECS + 60;
    vi.setSystemTime(later * 1_000);
    await syncChainClock(rpcReturning(BigInt(later) * 1_000n), 2n);

    // The +60 offset survives. Had the poisoned reading been accepted the
    // offset would be ~54,000 years and the gate astronomically large.
    expect(gateNowUnix()).toBe(later + 60 + CLOCK_SAFETY_MARGIN_SECS);
  });

  it("keeps a good offset when the node declines to time a slot", async () => {
    const { gateNowUnix, syncChainClock } = await import("./chainClock");

    await syncChainClock(rpcReturning(BigInt(DEVICE_SECS + 60)), 1n);
    const later = DEVICE_SECS + 60;
    vi.setSystemTime(later * 1_000);
    await syncChainClock(rpcReturning(null), 2n);

    expect(gateNowUnix()).toBe(later + 60 + CLOCK_SAFETY_MARGIN_SECS);
  });

  // The third leg of the equivalence the read path's comment claims: a
  // rejected reading, a slot the node won't time, and a transport error all
  // leave the previous offset standing.
  it("keeps a good offset when the transport throws", async () => {
    const { gateNowUnix, syncChainClock } = await import("./chainClock");

    await syncChainClock(rpcReturning(BigInt(DEVICE_SECS + 60)), 1n);
    const later = DEVICE_SECS + 60;
    vi.setSystemTime(later * 1_000);
    await syncChainClock(rpcThrowing(), 2n);

    expect(gateNowUnix()).toBe(later + 60 + CLOCK_SAFETY_MARGIN_SECS);
  });

  it("falls back to the device clock when no reading has landed", async () => {
    const { gateNowUnix, syncChainClock } = await import("./chainClock");

    await syncChainClock(rpcReturning(BigInt(DEVICE_SECS) * 1_000n), 1n);
    expect(gateNowUnix()).toBe(DEVICE_SECS + CLOCK_SAFETY_MARGIN_SECS);
  });
});

describe("slot gate", () => {
  it("nudges a confirmed slot forward by the slot-domain margin", async () => {
    const { gateNowSlot } = await import("./chainClock");

    expect(gateNowSlot(1_000)).toBe(1_000 + SLOT_SAFETY_MARGIN_SLOTS);
    expect(gateNowSlot(1_000n)).toBe(1_000 + SLOT_SAFETY_MARGIN_SLOTS);
  });

  // The whole point of the constant: a `confirmed` slot is behind head, so
  // gating on it raw reports levels as more alive than the engine will find
  // them. The margin must move the gate *forward* (levels die sooner here),
  // matching the direction the wall-clock margin moves.
  it("moves the gate forward, so a dying level is dropped sooner", async () => {
    const { gateNowSlot } = await import("./chainClock");

    expect(gateNowSlot(500)).toBeGreaterThan(500);
  });
});
