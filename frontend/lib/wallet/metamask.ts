import { PUBLIC_RPC_URL } from "../env";

// MetaMask Connect (the relay-based SDK, distinct from the MetaMask browser
// extension) only becomes discoverable once a dapp initializes it: calling
// createSolanaClient registers a Wallet Standard wallet into the global
// registry — the same registry our watchWalletStandardConnectors() in
// providers.tsx listens on — so it then flows through our normal discovery and
// shows up in the picker alongside extension wallets. Unlike injected wallets
// it works on mobile, in incognito, and with no extension installed.

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
    });
  })().catch((error) => {
    // A failed registration must not take down the rest of the wallet picker;
    // clear the latch so a later mount can retry.
    registration = null;
    console.warn("[wallet] MetaMask Connect registration failed", error);
  });
  return registration;
}
