"use client";

import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useEffect, useRef, useState } from "react";
import {
  fetchChainPositions,
  fetchChainVaultMarkets,
  pairKey,
} from "../data/chainVaults";
import type { VaultPosition } from "../data/positions";
import { VAULTS_REFRESH_MS } from "../data/timings";
import type { VaultMarket } from "../data/vaults";
import { useAppEvent } from "../events";

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
  const restartRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    let timer: number | undefined;
    let cancelled = false;
    let generation = 0;
    let lastGood: VaultMarket[] = [];
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
        const read = await fetchChainVaultMarkets(rpc);
        // A pair whose read failed this round keeps its last good market, so
        // one transient failure doesn't drop its rows (and the positions in
        // it) until the next tick.
        const markets = [
          ...read.markets,
          ...lastGood.filter((m) => read.failed.has(pairKey(m.base, m.quote))),
        ];
        lastGood = markets;
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

    // Restart the poll chain now: a new generation, so the old chain's
    // pending tick lapses instead of running alongside.
    const restart = () => {
      generation += 1;
      if (timer !== undefined) window.clearTimeout(timer);
      schedule(0, generation);
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") restart();
    };
    document.addEventListener("visibilitychange", onVisible);
    restartRef.current = restart;

    return () => {
      restartRef.current = null;
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [client, owner]);

  // A confirmed deposit or withdraw moved this vault's reserves and shares and
  // the wallet's position, so read now rather than up to a poll period later.
  useAppEvent("vaultTxSucceeded", () => restartRef.current?.());

  return state;
}
