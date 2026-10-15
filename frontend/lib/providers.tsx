"use client";

import {
  type ClientLogger,
  createClient,
  getWalletStandardConnectors,
  type SolanaClient,
  type WalletConnector,
  watchWalletStandardConnectors,
} from "@solana/client";
import { SolanaProvider } from "@solana/react-hooks";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { flushSync } from "react-dom";
import {
  CLUSTER,
  FALLBACK_RPC_URL,
  FALLBACK_WS_URL,
  PRIMARY_RPC_URL,
} from "./env";
import { useAppEvent } from "./events";
import {
  assertEndpointMatchesCluster,
  createAppRuntime,
  providerNote,
} from "./rpc";
import { registerMetaMaskConnect } from "./wallet/metamask";
import { registerMobileWalletAdapter } from "./wallet/mobile";

// Benign wallet-connect outcomes — the user dismissed the wallet modal or the
// relay/QR handshake timed out. Not errors worth surfacing; the UI already
// reflects the failed status.
const SILENCED_CONNECT_REASONS = new Set([
  "User closed modal",
  "User rejected the request",
  "Transport request timed out",
]);

// Route the client's logs to the console, but drop the benign connect outcomes
// above entirely and keep any other connection failure off console.error (so
// Next's dev error overlay doesn't flag an outcome already shown in the UI).
const logger: ClientLogger = ({ level, message, data }) => {
  if (message === "wallet connection failed") {
    const reason = typeof data?.message === "string" ? data.message : "";
    if (SILENCED_CONNECT_REASONS.has(reason)) return;
    console.warn(`[solana] ${message}`, data ?? {});
    return;
  }
  const fn =
    level === "error"
      ? console.error
      : level === "warn"
        ? console.warn
        : level === "info"
          ? console.info
          : console.debug;
  fn(`[solana] ${message}`, data ?? {});
};

// Fail fast on an endpoint that contradicts the cluster flag, and name the
// live provider once per page load.
if (typeof window !== "undefined") {
  assertEndpointMatchesCluster(
    "NEXT_PUBLIC_RPC_URL",
    FALLBACK_RPC_URL,
    CLUSTER,
  );
  if (PRIMARY_RPC_URL) {
    assertEndpointMatchesCluster(
      "NEXT_PUBLIC_PRIMARY_RPC_URL",
      PRIMARY_RPC_URL,
      CLUSTER,
    );
  }
  console.info(providerNote());
}

const makeClient = (connectors: readonly WalletConnector[]): SolanaClient => {
  // The standing endpoint here serves only createClient's own warm-up read,
  // which runs before the runtime below replaces it.
  const client = createClient({
    endpoint: FALLBACK_RPC_URL,
    websocketEndpoint: FALLBACK_WS_URL,
    walletConnectors: connectors,
    logger,
  });
  // Install the failover runtime over the single-endpoint one. Passing it as
  // `rpcClient` is not enough: createClient's own initial `setCluster` swaps
  // `runtime.rpc` for a fresh single-endpoint client before returning. Every
  // hook and the client's own actions read the shared `runtime` object, so
  // replacing its members here reaches all of them.
  Object.assign(client.runtime, createAppRuntime());
  return client;
};

// Identity of a connector set, independent of object identity, so we only
// rebuild the client when the available wallets actually change.
const connectorKey = (connectors: readonly WalletConnector[]): string =>
  connectors
    .map((c) => c.id)
    .sort()
    .join("|");

// Safe to swap the client without yanking a live connection out from under
// the user. We can't use useWallet() here (Providers sits above the provider
// that supplies its context), so read the wallet status off the client store
// directly.
const isIdle = (client: SolanaClient): boolean => {
  const status = client.store.getState().wallet.status;
  return status === "disconnected" || status === "error";
};

export function Providers({ children }: { children: ReactNode }) {
  // Wallet Standard wallets (Phantom, Backpack, …) register themselves with
  // the page asynchronously. On a cold browser start the extension often
  // injects *after* this module first evaluates, so a one-shot autoDiscover()
  // at load time misses it — and because the client's connector registry is
  // immutable, the wallet stays both absent from the picker and impossible to
  // connect (connect() throws "No wallet connector registered") until a manual
  // refresh. So we watch the registry and rebuild the client whenever the set
  // of available wallets changes.
  const [client, setClient] = useState<SolanaClient>(() =>
    makeClient(getWalletStandardConnectors()),
  );
  const clientRef = useRef(client);
  const keyRef = useRef(connectorKey(client.connectors.all));
  // A connector set seen while it wasn't safe to swap the client, deferred
  // until it is (see below). Null when there's nothing pending.
  const pendingRef = useRef<readonly WalletConnector[] | null>(null);
  // True while a connect is in flight. Swapping the client then races the
  // connect: the session lands on the old (destroyed) client while React
  // rebinds to the new one, so the header stays "Connect Wallet" until a
  // reload. The picker drives this flag via the `walletConnecting` event,
  // asserted synchronously from the row's click through connect-resolution
  // (the SolanaClient store doesn't reliably hold "connecting" during an
  // external SDK's relay flow, so isIdle alone wouldn't keep us safe past
  // `modal.close()`). An open-but-idle picker deliberately does NOT block a
  // swap: a wallet that registers late (MetaMask's relay on mobile) must
  // reach the picker the user is already looking at, or its row renders as
  // an install link until the picker is closed and reopened.
  const connectingRef = useRef(false);

  // `inEffect` marks a call made from inside a React effect, where React is
  // already committing and `flushSync` would only warn. Every other caller is
  // a registry or store callback outside React, where a plain `setClient`
  // would leave the destroyed client rendered for a moment: a picker row
  // clicked in that window connects on the dead client, so there the swap
  // commits synchronously.
  const rebuild = useCallback(
    (connectors: readonly WalletConnector[], inEffect: boolean) => {
      // SolanaProvider only auto-destroys a client it created itself; since
      // we pass our own, tear down the superseded one so its RPC
      // subscriptions don't leak.
      const next = makeClient(connectors);
      clientRef.current.destroy();
      clientRef.current = next;
      keyRef.current = connectorKey(connectors);
      if (inEffect) setClient(next);
      else flushSync(() => setClient(next));
    },
    [],
  );

  // Safe to swap the client only when no connection is live or in flight.
  const canRebuild = useCallback(
    (c: SolanaClient): boolean => isIdle(c) && !connectingRef.current,
    [],
  );

  // Apply a deferred connector set when it becomes safe. Reads clientRef so it
  // always targets the live client; callable from the store subscription and
  // the connect-settled event.
  const flush = useCallback(
    (inEffect: boolean) => {
      const c = clientRef.current;
      const pending = pendingRef.current;
      if (!pending || !canRebuild(c)) return;
      pendingRef.current = null;
      // The set may have churned back to what this client already has (a
      // wallet unregistered then re-registered); skip the needless rebuild.
      if (connectorKey(pending) === connectorKey(c.connectors.all)) return;
      rebuild(pending, inEffect);
    },
    [canRebuild, rebuild],
  );

  useAppEvent("walletConnecting", (connecting) => {
    connectingRef.current = connecting;
    // Connect just settled → a connector set deferred while it was in flight
    // can now be applied (if the wallet is also idle).
    if (!connecting) flush(false);
  });

  useEffect(() => {
    // Register MetaMask Connect and, on Android, the Mobile Wallet Adapter so
    // they join the Wallet Standard registry; the watcher below then surfaces
    // them like any other discovered wallet.
    void registerMetaMaskConnect();
    void registerMobileWalletAdapter();
  }, []);

  useEffect(() => {
    // watchWalletStandardConnectors emits the current set synchronously, then
    // again on every register/unregister; it returns the unsubscribe. Only
    // that first emission runs inside this effect.
    let subscribing = true;
    const unsubscribe = watchWalletStandardConnectors((connectors) => {
      const nextKey = connectorKey(connectors);
      if (nextKey === keyRef.current) return;
      keyRef.current = nextKey;
      // Rebuilding the client drops any active connection and races an
      // in-flight connect, so only swap when it's safe; otherwise defer until
      // the wallet is idle and no connect is in flight.
      if (canRebuild(clientRef.current)) {
        rebuild(connectors, subscribing);
      } else {
        pendingRef.current = connectors;
      }
    });
    subscribing = false;
    return unsubscribe;
  }, [canRebuild, rebuild]);

  useEffect(() => {
    // Flush a deferred connector set once it's safe. Re-subscribes whenever
    // `client` changes so we're always watching the live store.
    const unsubscribe = client.store.subscribe(() => flush(false));
    flush(true);
    return unsubscribe;
  }, [client, flush]);

  return <SolanaProvider client={client}>{children}</SolanaProvider>;
}
