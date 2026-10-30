// The poll loop behind every visibility-paused poller in the app: one timer,
// paused while the tab is hidden, restarted the moment it is shown again, and
// torn down with the effect that started it. Each poller supplies only its
// tick body, so the lifecycle has one copy to fix.

// What a tick asks for next: another tick after `intervalMs`, no further tick
// (until a restart, or the effect re-runs on an input change), or another
// tick after a delay of its own choosing, in milliseconds.
export type PollNext = "again" | "stop" | number;

export type PollChainOptions = {
  intervalMs: number;
  // Delay before the first tick: a debounce for an input-driven poller, zero
  // for one that should read at once.
  initialDelayMs: number;
  // One tick. `isLive` turns false once the chain is stopped or superseded by
  // a restart; check it after every await and bail out, since nothing a stale
  // tick does may reach state. Owns its own errors — a throw ends the chain.
  fire: (isLive: () => boolean) => Promise<PollNext>;
};

export type PollChain = {
  // Tick now, superseding any tick in flight. A no-op once stopped.
  restart: () => void;
  // Stop for good: the effect's cleanup.
  stop: () => void;
};

export const startPollChain = ({
  intervalMs,
  initialDelayMs,
  fire,
}: PollChainOptions): PollChain => {
  let timer: number | undefined;
  let stopped = false;
  // A monotonically-increasing id for the live tick→schedule chain. Only the
  // current generation may reschedule; a tick superseded by a restart (a
  // tab-refocus, say) sees a stale `gen` and drops its reschedule, so exactly
  // one timer stays live and the RPC cadence can't double.
  let generation = 0;

  const schedule = (delay: number, gen: number) => {
    if (stopped || gen !== generation) return;
    if (timer !== undefined) window.clearTimeout(timer);
    timer = window.setTimeout(() => void tick(gen), delay);
  };

  const tick = async (gen: number): Promise<void> => {
    const isLive = () => !stopped && gen === generation;
    if (!isLive()) return;
    // Pause when the tab is hidden, but keep the chain alive to resume.
    if (document.visibilityState !== "visible") {
      schedule(intervalMs, gen);
      return;
    }
    const next = await fire(isLive);
    if (!isLive() || next === "stop") return;
    schedule(next === "again" ? intervalMs : next, gen);
  };

  // Supersede any in-flight tick so its reschedule is dropped, then start a
  // fresh chain. Without the generation bump an in-flight tick whose timer
  // already elapsed would reschedule alongside this one, doubling cadence.
  const restart = () => {
    if (stopped) return;
    generation += 1;
    if (timer !== undefined) window.clearTimeout(timer);
    schedule(0, generation);
  };

  const onVisible = () => {
    if (document.visibilityState === "visible") restart();
  };

  schedule(initialDelayMs, generation);
  document.addEventListener("visibilitychange", onVisible);

  return {
    restart,
    stop: () => {
      stopped = true;
      if (timer !== undefined) window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    },
  };
};
