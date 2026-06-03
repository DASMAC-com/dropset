"use client";

import {
  createClient,
  getWalletStandardConnectors,
  type SolanaClient,
  type WalletConnector,
  watchWalletStandardConnectors,
} from "@solana/client";
import { SolanaProvider } from "@solana/react-hooks";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { PUBLIC_RPC_URL, PUBLIC_WS_URL } from "./env";

const makeClient = (connectors: readonly WalletConnector[]): SolanaClient =>
  createClient({
    endpoint: PUBLIC_RPC_URL,
    websocketEndpoint: PUBLIC_WS_URL,
    walletConnectors: connectors,
  });

// Identity of a connector set, independent of object identity, so we only
// rebuild the client when the available wallets actually change.
const connectorKey = (connectors: readonly WalletConnector[]): string =>
  connectors
    .map((c) => c.id)
    .sort()
    .join("|");

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

  useEffect(() => {
    // watchWalletStandardConnectors emits the current set synchronously, then
    // again on every register/unregister; it returns the unsubscribe.
    return watchWalletStandardConnectors((connectors) => {
      const nextKey = connectorKey(connectors);
      if (nextKey === keyRef.current) return;
      keyRef.current = nextKey;
      // SolanaProvider only auto-destroys a client it created itself; since we
      // pass our own, we tear down the superseded one so its RPC subscriptions
      // don't leak. Any active connection is restored by walletPersistence's
      // autoConnect against the freshly built client.
      const next = makeClient(connectors);
      clientRef.current.destroy();
      clientRef.current = next;
      setClient(next);
    });
  }, []);

  return <SolanaProvider client={client}>{children}</SolanaProvider>;
}
