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
import { type ReactNode, useEffect, useRef, useState } from "react";
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
import { type ClientSwap, createClientSwap } from "./wallet/clientSwap";
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
  // of available wallets changes, when it is safe to (see clientSwap.ts).
  const [client, setClient] = useState<SolanaClient>(() =>
    makeClient(getWalletStandardConnectors()),
  );
  const swapRef = useRef<ClientSwap<WalletConnector> | null>(null);
  if (!swapRef.current) {
    swapRef.current = createClientSwap(client, {
      make: makeClient,
      isIdle,
      commit: (next, inEffect) => {
        if (inEffect) setClient(next);
        else flushSync(() => setClient(next));
      },
    });
  }
  const swap = swapRef.current;

  // The picker asserts this from a row's click until the connect settles.
  useAppEvent("walletConnecting", (connecting) =>
    swap.setConnecting(connecting),
  );

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
    const unsubscribe = watchWalletStandardConnectors((connectors) =>
      swap.onConnectors(connectors, subscribing),
    );
    subscribing = false;
    return unsubscribe;
  }, [swap]);

  useEffect(() => {
    // Flush a deferred connector set once it's safe. Re-subscribes whenever
    // `client` changes so we're always watching the live store.
    const unsubscribe = client.store.subscribe(() => swap.flush(false));
    swap.flush(true);
    return unsubscribe;
  }, [client, swap]);

  return <SolanaProvider client={client}>{children}</SolanaProvider>;
}
