import { describe, expect, it } from "vitest";
import { type ClientSwap, connectorKey, createClientSwap } from "./clientSwap";

type Connector = { id: string };

// A stand-in client: its connector set, a wallet status, and a store whose
// subscribers fire synchronously on every write, as the real client's do.
// `destroy()` resets the store, which is the write that matters here: the old
// client's subscription is still live when it runs.
type FakeClient = {
  connectors: { all: readonly Connector[] };
  idle: boolean;
  destroyed: boolean;
  listeners: (() => void)[];
  destroy(): void;
};

const fakeClient = (ids: string[]): FakeClient => {
  const c: FakeClient = {
    connectors: { all: ids.map((id) => ({ id })) },
    idle: true,
    destroyed: false,
    listeners: [],
    destroy() {
      c.destroyed = true;
      c.idle = true;
      for (const l of c.listeners) l();
    },
  };
  return c;
};

const set = (...ids: string[]): Connector[] => ids.map((id) => ({ id }));

// Mirrors the wiring in providers.tsx: every client built gets a store
// subscription that flushes, and none is removed until React's effect cleanup
// would remove it, i.e. never within one synchronous call.
function harness(initialIds: string[] = ["a"]) {
  const initial = fakeClient(initialIds);
  const built: FakeClient[] = [];
  const commits: { client: FakeClient; inEffect: boolean }[] = [];
  let swap: ClientSwap<Connector> | undefined;
  const subscribe = (c: FakeClient) =>
    c.listeners.push(() => swap?.flush(false));
  subscribe(initial);
  swap = createClientSwap(initial, {
    make: (connectors) => {
      const c = fakeClient(connectors.map((k) => k.id));
      built.push(c);
      subscribe(c);
      return c;
    },
    isIdle: (c) => c.idle,
    commit: (client, inEffect) => commits.push({ client, inEffect }),
  });
  return { swap, initial, built, commits };
}

describe("connectorKey", () => {
  it("ignores order and object identity", () => {
    expect(connectorKey(set("b", "a"))).toEqual(connectorKey(set("a", "b")));
    expect(connectorKey(set("a"))).not.toEqual(connectorKey(set("a", "b")));
  });
});

describe("client swap", () => {
  it("ignores an emission of the set the client already has", () => {
    const h = harness(["a"]);
    h.swap.onConnectors(set("a"), true);
    expect(h.built).toHaveLength(0);
    expect(h.commits).toHaveLength(0);
  });

  it("rebuilds at once when idle, destroying the old client", () => {
    const h = harness(["a"]);
    h.swap.onConnectors(set("a", "b"), false);
    expect(h.built).toHaveLength(1);
    expect(h.initial.destroyed).toBe(true);
    expect(h.commits).toEqual([{ client: h.built[0], inEffect: false }]);
  });

  // The watcher's first emission runs inside the effect that subscribes it,
  // so it must commit with a plain setState, never flushSync.
  it("passes inEffect through to the commit", () => {
    const h = harness(["a"]);
    h.swap.onConnectors(set("a", "b"), true);
    expect(h.commits).toEqual([{ client: h.built[0], inEffect: true }]);
  });

  // The flush that applies it here is the one the client effect makes, so it
  // must commit with a plain setState too.
  it("defers while a connection is live, then applies it once idle", () => {
    const h = harness(["a"]);
    h.initial.idle = false;
    h.swap.onConnectors(set("a", "b"), false);
    expect(h.built).toHaveLength(0);
    h.swap.flush(false);
    expect(h.built).toHaveLength(0);
    h.initial.idle = true;
    h.swap.flush(true);
    expect(h.built.map((c) => connectorKey(c.connectors.all))).toEqual(["a|b"]);
    expect(h.commits).toEqual([{ client: h.built[0], inEffect: true }]);
    expect(h.initial.destroyed).toBe(true);
  });

  // A set applied directly supersedes one deferred earlier, so no later flush
  // may roll the client back to the stale set.
  it("drops a deferred set once a newer one is applied directly", () => {
    const h = harness(["a"]);
    h.initial.idle = false;
    h.swap.onConnectors(set("a", "b"), false);
    h.initial.idle = true;
    h.swap.onConnectors(set("a", "b", "c"), false);
    h.swap.flush(false);
    expect(h.built.map((c) => connectorKey(c.connectors.all))).toEqual([
      "a|b|c",
    ]);
  });

  // An idle store is not enough: an external SDK's relay flow can leave the
  // store idle while the connect is still in flight.
  it("defers while a connect is in flight even with an idle store", () => {
    const h = harness(["a"]);
    h.swap.setConnecting(true);
    h.swap.onConnectors(set("a", "b"), false);
    h.swap.flush(false);
    expect(h.built).toHaveLength(0);
    h.swap.setConnecting(false);
    expect(h.built).toHaveLength(1);
    expect(h.commits[0]?.inEffect).toBe(false);
  });

  it("applies only the latest of several deferred sets", () => {
    const h = harness(["a"]);
    h.swap.setConnecting(true);
    h.swap.onConnectors(set("a", "b"), false);
    h.swap.onConnectors(set("a", "b", "c"), false);
    h.swap.setConnecting(false);
    expect(h.built.map((c) => connectorKey(c.connectors.all))).toEqual([
      "a|b|c",
    ]);
  });

  it("drops a deferred set that churned back to the client's own", () => {
    const h = harness(["a"]);
    h.swap.setConnecting(true);
    h.swap.onConnectors(set("a", "b"), false);
    h.swap.onConnectors(set("a"), false);
    h.swap.setConnecting(false);
    expect(h.built).toHaveLength(0);
    // And it stays dropped: a later flush finds nothing pending.
    h.swap.flush(true);
    expect(h.built).toHaveLength(0);
  });

  // The open question from the picker rework's review: a store notification
  // fired synchronously inside a React effect would reach `flushSync` there.
  // The one such notification this module triggers itself is the old
  // client's reset in destroy(). Every rebuild clears the pending set before
  // it destroys, so that re-entrant flush finds nothing to do and the only
  // commit is the outer one, with the outer call's `inEffect`.
  it("does not re-enter on the old client's destroy notification", () => {
    const h = harness(["a"]);
    // Leave a stale set pending, then make the swap safe without flushing,
    // which is the shape a watcher emission can still meet.
    h.initial.idle = false;
    h.swap.onConnectors(set("a", "b"), false);
    h.initial.idle = true;
    h.swap.onConnectors(set("a", "b", "c"), true);
    expect(h.built).toHaveLength(1);
    expect(h.commits).toEqual([{ client: h.built[0], inEffect: true }]);
    expect(h.built[0]?.destroyed).toBe(false);
  });

  it("targets the live client after a swap", () => {
    const h = harness(["a"]);
    h.swap.onConnectors(set("a", "b"), false);
    const live = h.built[0];
    if (!live) throw new Error("no client built");
    live.idle = false;
    h.swap.onConnectors(set("a", "b", "c"), false);
    expect(h.built).toHaveLength(1);
    live.idle = true;
    h.swap.flush(false);
    expect(h.built).toHaveLength(2);
    expect(live.destroyed).toBe(true);
  });
});
