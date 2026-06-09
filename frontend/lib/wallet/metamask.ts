import { useSyncExternalStore } from "react";
import { PUBLIC_RPC_URL } from "../env";

// MetaMask Connect (the relay-based SDK, distinct from the MetaMask browser
// extension) only becomes discoverable once a dapp initializes it: calling
// createSolanaClient registers a Wallet Standard wallet into the global
// registry — the same registry our watchWalletStandardConnectors() in
// providers.tsx listens on — so it then flows through our normal discovery and
// shows up in the picker alongside extension wallets. Unlike injected wallets
// it works on mobile, in incognito, and with no extension installed.

type EthereumProvider = {
  isMetaMask?: boolean;
  providers?: EthereumProvider[];
};

/**
 * Whether the MetaMask *extension* is actually present in this browser. Used to
 * gate the picker's "Detected" badge: we don't want MetaMask to read as
 * detected merely because we registered its relay SDK. Client-only — returns
 * false during SSR. Handles the legacy `window.ethereum.providers` array (used
 * when several wallets inject) as well as a lone `window.ethereum`.
 */
export function isMetaMaskExtensionPresent(): boolean {
  if (typeof window === "undefined") return false;
  const eth = (window as unknown as { ethereum?: EthereumProvider }).ethereum;
  if (!eth) return false;
  if (Array.isArray(eth.providers)) {
    return eth.providers.some((p) => p?.isMetaMask === true);
  }
  return eth.isMetaMask === true;
}

let registration: Promise<void> | null = null;
let registrationReady = false;
const registrationListeners = new Set<() => void>();

const notifyRegistrationListeners = () => {
  for (const listener of registrationListeners) listener();
};

/**
 * Register MetaMask Connect with the Wallet Standard registry. Idempotent and
 * client-only: the SDK is dynamically imported so it never runs during SSR and
 * stays out of the main bundle, and the singleton promise guards against the
 * double registration that StrictMode mounts or client rebuilds would cause.
 */
export function registerMetaMaskConnect(): Promise<void> {
  if (registration) return registration;
  registration = (async () => {
    const { createSolanaClient } = await import("@metamask/connect-solana");
    await createSolanaClient({
      dapp: {
        name: "Dropset",
        url: window.location.origin,
        iconUrl: `${window.location.origin}/favicon.png`,
      },
      // Reuse the app's mainnet RPC so this doesn't need its own Infura key.
      api: { supportedNetworks: { mainnet: PUBLIC_RPC_URL } },
      // Analytics is on. The SDK ships only connection / wallet-action
      // counters (no wallet address, no IP, no PII) to MetaMask's analytics
      // endpoint, and only after the user actually triggers a flow — not on
      // init. Failed batches are caught and retried inside the SDK, so an ad
      // blocker won't surface as a console error here. Helps MetaMask spot
      // dapp-side connection regressions; cost to us is zero.
      analytics: { enabled: true },
    });
    registrationReady = true;
    notifyRegistrationListeners();
  })().catch((error) => {
    // A failed registration must not take down the rest of the wallet picker;
    // clear the latch so a later mount can retry. Stays unready so the row in
    // the picker won't promise a connect that we can't fulfill.
    registration = null;
    registrationReady = false;
    notifyRegistrationListeners();
    console.warn("[wallet] MetaMask Connect registration failed", error);
  });
  return registration;
}

const subscribeRegistration = (listener: () => void) => {
  registrationListeners.add(listener);
  return () => {
    registrationListeners.delete(listener);
  };
};

const getRegistrationReady = () => registrationReady;
// SSR has no concept of "ready"; the row is hidden behind a client-only
// readiness check anyway, but useSyncExternalStore requires a server snapshot.
const getRegistrationReadyServer = () => false;

/**
 * Whether `registerMetaMaskConnect()`'s dynamic-imported SDK has finished
 * `createSolanaClient(...)`. Until this returns `true`, MetaMask's relay
 * connector either isn't in the Wallet Standard registry yet or has just
 * landed — a tap that races registration would either find no connector or
 * land on a partially-initialized one. The picker uses this to disable the
 * MetaMask row (and show a spinner) so the user doesn't fire a connect that
 * the SDK can't yet honor.
 */
export function useMetaMaskRegistrationReady(): boolean {
  return useSyncExternalStore(
    subscribeRegistration,
    getRegistrationReady,
    getRegistrationReadyServer,
  );
}
