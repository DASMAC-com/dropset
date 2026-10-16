// When it is safe to swap the SolanaClient for one built over a new wallet
// connector set, and what to do with a set seen while it isn't. Kept free of
// React so the decision can be unit-tested; providers.tsx wires it to the
// client state and the registry watcher.
//
// The client's connector registry is immutable, so a wallet that registers
// after the client was built (an extension injecting late on a cold start,
// MetaMask's relay, the Mobile Wallet Adapter) only becomes connectable on a
// fresh client. Swapping drops any live connection and races an in-flight
// connect, though, so a set seen while the wallet is busy is deferred until it
// is idle.

// The slice of a SolanaClient this module reads. Generic over the client and
// connector types so a test can drive it with plain objects.
export type SwappableClient<K extends { id: string }> = {
  readonly connectors: { readonly all: readonly K[] };
  destroy(): void;
};

export type ClientSwapDeps<K, C> = {
  make: (connectors: readonly K[]) => C;
  // True when no connection is live: disconnected, or a failed connect.
  isIdle: (client: C) => boolean;
  // Hand the new client to React. `inEffect` marks a call made from inside a
  // React effect, where React is already committing and `flushSync` would
  // only warn. Every other caller is a registry or store callback outside
  // React, where a deferred render would leave the destroyed client rendered
  // for a moment: a picker row clicked in that window connects on the dead
  // client, so there the swap commits synchronously.
  commit: (client: C, inEffect: boolean) => void;
};

export type ClientSwap<K> = {
  // A registry emission: the full current connector set.
  onConnectors: (connectors: readonly K[], inEffect: boolean) => void;
  // Apply a deferred set if it is now safe. Called from the live client's
  // store subscription, from the effect that installs it, and when a connect
  // settles.
  flush: (inEffect: boolean) => void;
  // Whether a connect is in flight. A swap then races the connect: the
  // session lands on the old (destroyed) client while React rebinds to the
  // new one, so the header stays "Connect Wallet" until a reload. The picker
  // asserts this synchronously from the row's click through
  // connect-resolution, because the client store does not reliably hold
  // "connecting" during an external SDK's relay flow, so `isIdle` alone would
  // not keep a swap out past `modal.close()`. An open-but-idle picker
  // deliberately does NOT block a swap: a wallet that registers late
  // (MetaMask's relay on mobile) must reach the picker the user is already
  // looking at, or its row renders as an install link until the picker is
  // closed and reopened.
  setConnecting: (connecting: boolean) => void;
};

// Identity of a connector set, independent of object identity, so the client
// is only rebuilt when the available wallets actually change.
export const connectorKey = (connectors: readonly { id: string }[]): string =>
  connectors
    .map((c) => c.id)
    .sort()
    .join("|");

export function createClientSwap<
  K extends { id: string },
  C extends SwappableClient<K>,
>(initial: C, deps: ClientSwapDeps<K, C>): ClientSwap<K> {
  let client = initial;
  // The last set the registry reported, applied or deferred.
  let seenKey = connectorKey(initial.connectors.all);
  // A set seen while it wasn't safe to swap; null when nothing is pending.
  let pending: readonly K[] | null = null;
  let connecting = false;

  const canRebuild = (): boolean => deps.isIdle(client) && !connecting;

  const rebuild = (connectors: readonly K[], inEffect: boolean): void => {
    // Whatever was deferred is superseded by this set. Clear it, and point at
    // the new client, BEFORE destroying the old one: destroy() resets the old
    // store, which notifies its subscribers synchronously, `flush` among them.
    // With a stale set still pending, that re-entrant flush would build a
    // second client and commit it through `flushSync` from wherever this call
    // stands (inside an effect, for the watcher's first emission), and the
    // outer call would then overwrite it without destroying it.
    pending = null;
    const prev = client;
    client = deps.make(connectors);
    seenKey = connectorKey(connectors);
    // SolanaProvider only auto-destroys a client it created itself; ours is
    // passed in, so tear down the superseded one or its RPC subscriptions
    // leak.
    prev.destroy();
    deps.commit(client, inEffect);
  };

  const flush = (inEffect: boolean): void => {
    if (!pending || !canRebuild()) return;
    const next = pending;
    pending = null;
    // The set may have churned back to what this client already has (a
    // wallet unregistered then re-registered); skip the needless rebuild.
    if (connectorKey(next) === connectorKey(client.connectors.all)) return;
    rebuild(next, inEffect);
  };

  return {
    onConnectors: (connectors, inEffect) => {
      const key = connectorKey(connectors);
      if (key === seenKey) return;
      seenKey = key;
      if (canRebuild()) rebuild(connectors, inEffect);
      else pending = connectors;
    },
    flush,
    setConnecting: (value) => {
      connecting = value;
      // A connect just settled: a set deferred while it was in flight can now
      // be applied, if the wallet is also idle.
      if (!value) flush(false);
    },
  };
}
