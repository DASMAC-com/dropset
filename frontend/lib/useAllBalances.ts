// cspell:word atas
"use client";

import { type Address, address } from "@solana/kit";
import { useSolanaClient, useWallet } from "@solana/react-hooks";
import {
  findAssociatedTokenPda,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";
import { useEffect, useSyncExternalStore } from "react";
import { ALL_STABLECOINS, type TokenProgramKind } from "./currencies";
import { useAppEvent } from "./events";

// One shared map per page load, keyed by mint string. Semantics:
//   undefined → not yet fetched / disconnected
//   null      → no associated token account
//   0n        → ATA exists but balance is zero
//   > 0n      → balance in atomic units
type MaybeBalance = bigint | null | undefined;

const balances = new Map<string, bigint | null>();
let fetchedForOwner: string | null = null;
// Monotonic counter so concurrent fetches don't race a stale write into the
// cache. Each fetch reads `++counter`; only the latest one writes.
let requestCounter = 0;
// In-flight dedupe keyed by owner address. Multiple consumers (the swap
// panel, picker rows, etc.) all subscribe via useAllBalances on mount and
// each fire their own `useEffect → fetchBalances` — without this, that
// fans out into N parallel RPCs for the same data. `swapSucceeded` calls
// pass `force: true` so a stale in-flight fetch can't satisfy them.
const inFlightByOwner = new Map<string, Promise<void>>();
const listeners = new Set<() => void>();
let version = 0;
const bump = () => {
  version++;
  for (const cb of listeners) cb();
};
const subscribe = (cb: () => void): (() => void) => {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
};
const getVersion = (): number => version;

const PROGRAM_FOR_KIND: Record<TokenProgramKind, Address> = {
  classic: TOKEN_PROGRAM_ADDRESS,
  token2022: TOKEN_2022_PROGRAM_ADDRESS,
};

// Derive every stablecoin's ATA for the given owner. Order matches
// ALL_STABLECOINS so we can re-index parsed accounts back to mints.
async function deriveAtas(owner: Address): Promise<Address[]> {
  return Promise.all(
    ALL_STABLECOINS.map(async (s) => {
      const [ata] = await findAssociatedTokenPda({
        owner,
        mint: address(s.mint),
        tokenProgram: PROGRAM_FOR_KIND[s.tokenProgram],
      });
      return ata;
    }),
  );
}

type ParsedTokenInfo = {
  tokenAmount?: { amount?: string };
};
type JsonParsedData = {
  parsed?: { info?: ParsedTokenInfo };
};

function fetchBalances(
  rpc: ReturnType<typeof useSolanaClient>["runtime"]["rpc"],
  ownerStr: string,
  opts: { force?: boolean } = {},
): Promise<void> {
  // Non-force callers attach to a same-owner fetch already in flight so
  // initial-mount fan-out collapses to one RPC round-trip. Force callers
  // (e.g. swap-success refresh) always start a new request and overwrite
  // the entry; the requestCounter stale-guard below stops the older fetch
  // from clobbering the cache once the force fetch lands.
  if (!opts.force) {
    const existing = inFlightByOwner.get(ownerStr);
    if (existing) return existing;
  }

  const promise = (async () => {
    const my = ++requestCounter;
    const owner = address(ownerStr);
    const atas = await deriveAtas(owner);
    const res = await rpc
      .getMultipleAccounts(atas, {
        encoding: "jsonParsed",
        commitment: "confirmed",
      })
      .send();
    if (my !== requestCounter) return;

    res.value.forEach((account, i) => {
      const mint = ALL_STABLECOINS[i].mint;
      if (!account) {
        balances.set(mint, null);
        return;
      }
      // jsonParsed gives us `data.parsed.info.tokenAmount.amount` as a
      // stringified bigint when the RPC recognized this as a token-account.
      const parsed = (account.data as JsonParsedData)?.parsed?.info;
      const amount = parsed?.tokenAmount?.amount;
      balances.set(mint, amount != null ? BigInt(amount) : 0n);
    });
    fetchedForOwner = ownerStr;
    bump();
  })().finally(() => {
    // Only clear if a newer (force) fetch hasn't replaced our entry already.
    if (inFlightByOwner.get(ownerStr) === promise) {
      inFlightByOwner.delete(ownerStr);
    }
  });
  inFlightByOwner.set(ownerStr, promise);
  return promise;
}

export type UseAllBalances = {
  // null → no ATA, 0n → empty account, > 0n → balance, undefined → loading
  balanceFor: (mint: string) => MaybeBalance;
  isReady: boolean;
  refresh: () => void;
};

export function useAllBalances(): UseAllBalances {
  const wallet = useWallet();
  const client = useSolanaClient();
  const ownerStr =
    wallet.status === "connected"
      ? wallet.session.account.address.toString()
      : null;

  // Re-render any consumer when the cache changes.
  useSyncExternalStore(subscribe, getVersion, getVersion);

  // Kick a fetch on connect (or on wallet swap). Disconnect wipes the cache so
  // a reconnect to a different wallet doesn't briefly show the prior owner's
  // balances.
  useEffect(() => {
    if (!ownerStr) {
      if (fetchedForOwner !== null) {
        balances.clear();
        fetchedForOwner = null;
        // Bumping `requestCounter` cancels any in-flight fetch from the prior
        // owner so its stale write can't land after the clear.
        requestCounter++;
        bump();
      }
      return;
    }
    if (fetchedForOwner !== ownerStr) {
      void fetchBalances(client.runtime.rpc, ownerStr);
    }
  }, [ownerStr, client]);

  // After a swap lands, re-fetch — once now and once ~1.5 s later to absorb
  // RPC propagation lag between confirmation status and account state.
  // Snapshot `ownerStr` so the delayed call still targets the right wallet
  // even if the user has disconnected by then (no-op rather than crash).
  useAppEvent("swapSucceeded", () => {
    const owner = ownerStr;
    if (!owner) return;
    void fetchBalances(client.runtime.rpc, owner, { force: true });
    window.setTimeout(() => {
      void fetchBalances(client.runtime.rpc, owner, { force: true });
    }, 1500);
  });

  return {
    balanceFor: (mint: string): MaybeBalance =>
      ownerStr === null ? undefined : balances.get(mint),
    isReady: ownerStr !== null && fetchedForOwner === ownerStr,
    refresh: () => {
      if (ownerStr) void fetchBalances(client.runtime.rpc, ownerStr);
    },
  };
}
