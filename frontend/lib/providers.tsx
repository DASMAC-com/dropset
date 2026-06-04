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
import { PUBLIC_RPC_URL, PUBLIC_WS_URL } from "./env";
import { registerMetaMaskConnect } from "./wallet/metamask";

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

const makeClient = (connectors: readonly WalletConnector[]): SolanaClient =>
  createClient({
    endpoint: PUBLIC_RPC_URL,
    websocketEndpoint: PUBLIC_WS_URL,
    walletConnectors: connectors,
    logger,
  });

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
  // A connector set seen while a wallet was connected, deferred until the
  // user disconnects (see below). Null when there's nothing pending.
  const pendingRef = useRef<readonly WalletConnector[] | null>(null);

  const rebuild = useCallback((connectors: readonly WalletConnector[]) => {
    // SolanaProvider only auto-destroys a client it created itself; since we
    // pass our own, tear down the superseded one so its RPC subscriptions
    // don't leak.
    const next = makeClient(connectors);
    clientRef.current.destroy();
    clientRef.current = next;
    keyRef.current = connectorKey(connectors);
    setClient(next);
  }, []);

  useEffect(() => {
    // Register MetaMask Connect so it joins the Wallet Standard registry; the
    // watcher below then surfaces it like any other discovered wallet.
    void registerMetaMaskConnect();
  }, []);

  useEffect(() => {
    // watchWalletStandardConnectors emits the current set synchronously, then
    // again on every register/unregister; it returns the unsubscribe.
    return watchWalletStandardConnectors((connectors) => {
      const nextKey = connectorKey(connectors);
      if (nextKey === keyRef.current) return;
      keyRef.current = nextKey;
      // Rebuilding the client drops any active connection. Doing that the
      // instant some *other* wallet registers (e.g. Solflare loading a beat
      // after you've already connected Phantom) is a jarring flicker for
      // multi-wallet users, so defer the swap until they next disconnect.
      if (isIdle(clientRef.current)) {
        rebuild(connectors);
      } else {
        pendingRef.current = connectors;
      }
    });
  }, [rebuild]);

  useEffect(() => {
    // Flush a deferred connector set once the wallet goes idle. Re-subscribes
    // whenever `client` changes so we're always watching the live store.
    const flush = () => {
      const pending = pendingRef.current;
      if (!pending || !isIdle(client)) return;
      pendingRef.current = null;
      // The set may have churned back to what this client already has (a
      // wallet unregistered then re-registered); skip the needless rebuild.
      if (connectorKey(pending) === connectorKey(client.connectors.all)) return;
      rebuild(pending);
    };
    const unsubscribe = client.store.subscribe(flush);
    flush();
    return unsubscribe;
  }, [client, rebuild]);

  return <SolanaProvider client={client}>{children}</SolanaProvider>;
}
