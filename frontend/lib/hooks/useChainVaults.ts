"use client";

import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useEffect, useRef, useState } from "react";
import {
  fetchChainPositions,
  fetchChainVaultMarkets,
  pairKey,
} from "../data/chainVaults";
import type { VaultPosition } from "../data/positions";
import { BALANCE_REFETCH_DELAY_MS, VAULTS_REFRESH_MS } from "../data/timings";
import type { VaultMarket } from "../data/vaults";
import { useAppEvent } from "../events";
import { type PollNext, startPollChain } from "./pollChain";

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
// The loop is the shared poll chain (lib/hooks/pollChain.ts): paused while
// the tab is hidden, and guarded so a refocus cannot leave two chains
// running. It self-heals through a transient RPC error.
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
    let lastGood: VaultMarket[] = [];
    const rpc = client.runtime.rpc;

    const fire = async (isLive: () => boolean): Promise<PollNext> => {
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
        if (!isLive()) return "stop";
        setState((prev) => ({
          status: "ready",
          markets,
          positions: positions ?? prev.positions,
        }));
      } catch {
        if (!isLive()) return "stop";
        // Checked against state, not a flag local to this effect, so a
        // wallet switch (which re-runs it) cannot wipe a table that loaded.
        setState((prev) =>
          prev.status === "ready" ? prev : { ...INITIAL, status: "error" },
        );
      }
      return "again";
    };

    // A wallet switch re-keys the positions; drop the previous wallet's rows
    // rather than showing them under the new one until the next read lands.
    setState((prev) => ({ ...prev, positions: new Map() }));
    const chain = startPollChain({
      intervalMs: VAULTS_REFRESH_MS,
      initialDelayMs: 0,
      fire,
    });

    // After a confirmed transaction: read now, and once more after the same
    // short delay the balance cache waits out, since the node can report a
    // signature confirmed before its account reads reflect it.
    let followUp: number | undefined;
    restartRef.current = () => {
      chain.restart();
      if (followUp !== undefined) window.clearTimeout(followUp);
      followUp = window.setTimeout(() => {
        followUp = undefined;
        chain.restart();
      }, BALANCE_REFETCH_DELAY_MS);
    };

    return () => {
      restartRef.current = null;
      if (followUp !== undefined) window.clearTimeout(followUp);
      chain.stop();
    };
  }, [client, owner]);

  // A confirmed deposit or withdraw moved this vault's reserves and shares and
  // the wallet's position, so read now (and once more shortly after) rather
  // than up to a poll period later.
  useAppEvent("vaultTxSucceeded", () => restartRef.current?.());

  return state;
}
