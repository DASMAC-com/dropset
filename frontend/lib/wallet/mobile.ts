// Mobile wallet reach. A phone browser has no extension wallets to inject, so
// the desktop discovery path finds nothing there. Two routes fill the gap:
//
// - Android: the Solana Mobile Wallet Adapter (MWA) registers a Wallet
//   Standard wallet that hands the connect and every signature to whichever
//   installed wallet app claims the MWA intent (Phantom, Solflare, Backpack,
//   …), then returns to this tab. It flows through the same registry watcher
//   in providers.tsx as any extension.
// - iOS has no MWA, so the reliable route is the wallet's own in-app browser,
//   where the wallet injects itself and connects like a desktop extension.
//   `inWalletBrowseUrl` builds the universal link that reopens this page
//   there.

let registration: Promise<void> | null = null;

/**
 * Register the Mobile Wallet Adapter with the Wallet Standard registry.
 * Idempotent and client-only: the package is dynamically imported so it never
 * runs during SSR and stays out of the main bundle. `registerMwa` itself
 * no-ops outside a supported environment (anything but Android Chrome), so
 * calling this unconditionally is safe.
 */
export function registerMobileWalletAdapter(): Promise<void> {
  if (registration) return registration;
  registration = (async () => {
    const {
      createDefaultAuthorizationCache,
      createDefaultChainSelector,
      createDefaultWalletNotFoundHandler,
      registerMwa,
    } = await import("@solana-mobile/wallet-standard-mobile");
    registerMwa({
      appIdentity: {
        name: "Dropset",
        uri: window.location.origin,
        icon: "/favicon.png",
      },
      authorizationCache: createDefaultAuthorizationCache(),
      // The app only ever talks to mainnet (see PUBLIC_RPC_URL).
      chains: ["solana:mainnet"],
      chainSelector: createDefaultChainSelector(),
      onWalletNotFound: createDefaultWalletNotFoundHandler(),
    });
  })().catch((error) => {
    // Same posture as MetaMask's registration: a failure must not take down
    // the picker, and clearing the latch lets a later mount retry.
    registration = null;
    console.warn("[wallet] Mobile Wallet Adapter registration failed", error);
  });
  return registration;
}

/**
 * The universal link that opens `pageUrl` inside a wallet's in-app browser,
 * or undefined when the wallet publishes no such link. `browsePrefix` is the
 * curated entry's `browse` field (wallets.json); Phantom, Solflare and
 * Backpack all document the same `<prefix><url>?ref=<ref>` shape, with both
 * values URL-encoded and `ref` naming the requesting app.
 */
export function inWalletBrowseUrl(
  browsePrefix: string | undefined,
  pageUrl: string,
  ref: string,
): string | undefined {
  if (!browsePrefix) return undefined;
  return `${browsePrefix}${encodeURIComponent(pageUrl)}?ref=${encodeURIComponent(ref)}`;
}
