"use client";

// cspell:word sessioning

import { useDisconnectWallet, useWallet } from "@solana/react-hooks";
import { useEffect } from "react";

/**
 * Disconnect when the wallet's active account changes out from under us.
 *
 * @solana/client wires up `onAccountsChanged`, but its handler only reacts to
 * the *empty* case (all accounts revoked → disconnect). Switching to a
 * different account inside the extension leaves the store pinned to the old
 * address, so the picker, balances, and swap signer all silently go stale.
 *
 * Re-sessioning in place is fiddly — the wallet-standard session captures its
 * account by value — so we take the simple, predictable route: drop the
 * connection and let the user reconnect with whichever account they actually
 * want active. The next connect() picks up the new account cleanly.
 */
export function useWalletAccountWatch(): void {
  const wallet = useWallet();
  const disconnect = useDisconnectWallet();

  useEffect(() => {
    if (wallet.status !== "connected") return;
    const { session } = wallet;
    if (!session.onAccountsChanged) return;

    const connectedAddress = session.account.address.toString();
    return session.onAccountsChanged((accounts) => {
      const next = accounts[0]?.address?.toString();
      // A no-op event that still points at the connected account: ignore it.
      // Anything else — a switch to a different account, or the account being
      // revoked entirely — drops the session.
      if (next === connectedAddress) return;
      void disconnect().catch(() => {});
    });
  }, [wallet, disconnect]);
}
