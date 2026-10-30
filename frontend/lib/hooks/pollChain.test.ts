import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { type PollNext, startPollChain } from "./pollChain";

// A `document` with just the surface the chain touches, so the visibility
// transitions can be driven by hand.
const fakeDocument = () => {
  const listeners = new Set<() => void>();
  const doc = {
    visibilityState: "visible" as "visible" | "hidden",
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    listenerCount: () => listeners.size,
    setVisibility: (state: "visible" | "hidden") => {
      doc.visibilityState = state;
      for (const fn of listeners) fn();
    },
  };
  return doc;
};

const INTERVAL = 1_000;

describe("poll chain", () => {
  let doc: ReturnType<typeof fakeDocument>;

  beforeEach(() => {
    vi.useFakeTimers();
    doc = fakeDocument();
    vi.stubGlobal("document", doc);
    vi.stubGlobal("window", globalThis);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  const start = (fire: (isLive: () => boolean) => Promise<PollNext>) =>
    startPollChain({ intervalMs: INTERVAL, initialDelayMs: 50, fire });

  it("waits out the initial delay, then ticks on the interval", async () => {
    const fire = vi.fn(async (): Promise<PollNext> => "again");
    start(fire);

    await vi.advanceTimersByTimeAsync(49);
    expect(fire).toHaveBeenCalledTimes(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(fire).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(INTERVAL);
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it("ends on stop and honours a tick's own delay", async () => {
    const answers: PollNext[] = [5_000, "stop"];
    const fire = vi.fn(async () => answers.shift() ?? "again");
    start(fire);

    await vi.advanceTimersByTimeAsync(50);
    expect(fire).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(INTERVAL);
    expect(fire).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(5_000 - INTERVAL);
    expect(fire).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(10 * INTERVAL);
    expect(fire).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("pauses while hidden and ticks at once when shown", async () => {
    const fire = vi.fn(async (): Promise<PollNext> => "again");
    doc.visibilityState = "hidden";
    start(fire);

    await vi.advanceTimersByTimeAsync(10 * INTERVAL);
    expect(fire).toHaveBeenCalledTimes(0);
    // Still alive, waiting to resume.
    expect(vi.getTimerCount()).toBe(1);

    doc.setVisibility("visible");
    await vi.advanceTimersByTimeAsync(0);
    expect(fire).toHaveBeenCalledTimes(1);
  });

  // The regression the generation id exists for: a refocus while a tick is
  // awaiting must not leave the stale tick's reschedule running beside the
  // fresh chain, doubling the cadence.
  it("keeps one chain live across a refocus mid-tick", async () => {
    let release: (() => void) | undefined;
    const seenLive: boolean[] = [];
    const fire = vi.fn(async (isLive: () => boolean): Promise<PollNext> => {
      if (fire.mock.calls.length === 1) {
        await new Promise<void>((r) => {
          release = r;
        });
        seenLive.push(isLive());
      }
      return "again";
    });
    start(fire);

    await vi.advanceTimersByTimeAsync(50);
    expect(fire).toHaveBeenCalledTimes(1);
    doc.setVisibility("visible");
    await vi.advanceTimersByTimeAsync(0);
    expect(fire).toHaveBeenCalledTimes(2);

    release?.();
    await vi.advanceTimersByTimeAsync(0);
    expect(seenLive).toEqual([false]);
    expect(vi.getTimerCount()).toBe(1);

    await vi.advanceTimersByTimeAsync(INTERVAL);
    expect(fire).toHaveBeenCalledTimes(3);
  });

  // What every hook's state guard leans on: a tick still awaiting when its
  // effect is torn down must read as dead, or it writes into the next one.
  it("marks an in-flight tick dead once stopped", async () => {
    let release: (() => void) | undefined;
    const seenLive: boolean[] = [];
    const fire = vi.fn(async (isLive: () => boolean): Promise<PollNext> => {
      await new Promise<void>((r) => {
        release = r;
      });
      seenLive.push(isLive());
      return "again";
    });
    const chain = start(fire);

    await vi.advanceTimersByTimeAsync(50);
    chain.stop();
    release?.();
    await vi.advanceTimersByTimeAsync(10 * INTERVAL);
    expect(seenLive).toEqual([false]);
    expect(fire).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("pauses a running chain when the tab is hidden", async () => {
    const fire = vi.fn(async (): Promise<PollNext> => "again");
    start(fire);

    await vi.advanceTimersByTimeAsync(50);
    expect(fire).toHaveBeenCalledTimes(1);
    doc.setVisibility("hidden");
    await vi.advanceTimersByTimeAsync(10 * INTERVAL);
    expect(fire).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(1);
  });

  it("goes quiet on stop, and a later restart is a no-op", async () => {
    const fire = vi.fn(async (): Promise<PollNext> => "again");
    const chain = start(fire);

    await vi.advanceTimersByTimeAsync(50);
    chain.stop();
    expect(doc.listenerCount()).toBe(0);
    chain.restart();
    await vi.advanceTimersByTimeAsync(10 * INTERVAL);
    expect(fire).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });
});
