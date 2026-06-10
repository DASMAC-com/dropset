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
import { useAppEvent } from "./events";
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
  // A connector set seen while it wasn't safe to swap the client, deferred
  // until it is (see below). Null when there's nothing pending.
  const pendingRef = useRef<readonly WalletConnector[] | null>(null);
  // True while the user is mid-pick OR a connect is in flight. Swapping the
  // client during either races the connect: the session lands on the old
  // (destroyed) client while React rebinds to the new one, so the header
  // stays "Connect Wallet" until a reload. The picker drives this flag via
  // the `walletPickerOpen` event, which is asserted from picker-open through
  // connect-resolution (the SolanaClient store doesn't reliably hold
  // "connecting" during an external SDK's relay flow, so isIdle alone
  // wouldn't keep us safe past `modal.close()`).
  const pickerOpenRef = useRef(false);

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

  // Safe to swap the client only when no connection is live or in flight AND
  // the picker isn't open (the user could be about to click a wallet).
  const canRebuild = useCallback(
    (c: SolanaClient): boolean => isIdle(c) && !pickerOpenRef.current,
    [],
  );

  // Apply a deferred connector set when it becomes safe. Reads clientRef so it
  // always targets the live client; callable from the store subscription and
  // the picker-close event.
  const flush = useCallback(() => {
    const c = clientRef.current;
    const pending = pendingRef.current;
    if (!pending || !canRebuild(c)) return;
    pendingRef.current = null;
    // The set may have churned back to what this client already has (a wallet
    // unregistered then re-registered); skip the needless rebuild.
    if (connectorKey(pending) === connectorKey(c.connectors.all)) return;
    rebuild(pending);
  }, [canRebuild, rebuild]);

  useAppEvent("walletPickerOpen", (open) => {
    pickerOpenRef.current = open;
    // Picker just closed → a connector set deferred while it was open can now
    // be applied (if the wallet is also idle).
    if (!open) flush();
  });

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
      // Rebuilding the client drops any active connection and races an
      // in-flight connect, so only swap when it's safe; otherwise defer until
      // the wallet is idle and the picker is closed.
      if (canRebuild(clientRef.current)) {
        rebuild(connectors);
      } else {
        pendingRef.current = connectors;
      }
    });
  }, [canRebuild, rebuild]);

  useEffect(() => {
    // Flush a deferred connector set once it's safe. Re-subscribes whenever
    // `client` changes so we're always watching the live store.
    const unsubscribe = client.store.subscribe(flush);
    flush();
    return unsubscribe;
  }, [client, flush]);

  return <SolanaProvider client={client}>{children}</SolanaProvider>;
}
