"use client";

import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import {
  fetchChainPositions,
  fetchChainVaultMarkets,
} from "../data/chainVaults";
import type { VaultPosition } from "../data/positions";
import { VAULTS_REFRESH_MS } from "../data/timings";
import type { VaultMarket } from "../data/vaults";

export type ChainVaultsState = {
  // "loading" until the first read lands; "error" only when no read has ever
  // succeeded (a later failure keeps the last good table on screen).
  status: "loading" | "ready" | "error";
  markets: VaultMarket[];
  // The connected wallet's positions, keyed by vault id. Empty when no
  // wallet is connected.
  positions: Map<string, VaultPosition>;
};

const INITIAL: ChainVaultsState = {
  status: "loading",
  markets: [],
  positions: new Map(),
};

// Poll the live vaults and, with a wallet connected, its positions in them.
// Same lifecycle as useOrderBook: paused while the tab is hidden, self-healing
// through a transient RPC error, and generation-guarded so a refocus cannot
// leave two poll chains running.
export function useChainVaults(): ChainVaultsState {
  const client = useSolanaClient();
  const wallet = useWallet();
  const owner =
    wallet.status === "connected"
      ? wallet.session.account.address.toString()
      : null;
  const [state, setState] = useState<ChainVaultsState>(INITIAL);

  useEffect(() => {
    let timer: number | undefined;
    let cancelled = false;
    let generation = 0;
    const rpc = client.runtime.rpc;

    const schedule = (delay: number, gen: number) => {
      if (cancelled || gen !== generation) return;
      if (timer !== undefined) window.clearTimeout(timer);
      timer = window.setTimeout(() => void fire(gen), delay);
    };

    const fire = async (gen: number): Promise<void> => {
      if (cancelled || gen !== generation) return;
      if (document.visibilityState !== "visible") {
        schedule(VAULTS_REFRESH_MS, gen);
        return;
      }
      try {
        const markets = await fetchChainVaultMarkets(rpc);
        // The positions read fails on its own: the markets still land, and
        // this wallet's last good positions stay up until the next tick.
        let positions: Map<string, VaultPosition> | null = new Map();
        if (owner) {
          try {
            positions = await fetchChainPositions(rpc, owner, markets);
          } catch {
            positions = null;
          }
        }
        if (cancelled || gen !== generation) return;
        setState((prev) => ({
          status: "ready",
          markets,
          positions: positions ?? prev.positions,
        }));
      } catch {
        if (cancelled || gen !== generation) return;
        // Checked against state, not a flag local to this effect, so a
        // wallet switch (which re-runs it) cannot wipe a table that loaded.
        setState((prev) =>
          prev.status === "ready" ? prev : { ...INITIAL, status: "error" },
        );
      }
      schedule(VAULTS_REFRESH_MS, gen);
    };

    // A wallet switch re-keys the positions; drop the previous wallet's rows
    // rather than showing them under the new one until the next read lands.
    setState((prev) => ({ ...prev, positions: new Map() }));
    schedule(0, generation);

    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      generation += 1;
      if (timer !== undefined) window.clearTimeout(timer);
      schedule(0, generation);
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [client, owner]);

  return state;
}
