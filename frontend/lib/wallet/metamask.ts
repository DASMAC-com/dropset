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
 * false during SSR. Handles the EIP-5749 `providers` array (multiple injected
 * wallets) as well as a lone `window.ethereum`.
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
      // The SDK enables dapp-side telemetry by default and POSTs to MetaMask's
      // analytics endpoint on init; when that request is blocked (ad/tracker
      // blockers) it surfaces as an uncaught "Failed to fetch" in the console.
      // A DEX has no business firing MetaMask telemetry regardless, so off.
      analytics: { enabled: false },
    });
  })().catch((error) => {
    // A failed registration must not take down the rest of the wallet picker;
    // clear the latch so a later mount can retry.
    registration = null;
    console.warn("[wallet] MetaMask Connect registration failed", error);
  });
  return registration;
}
