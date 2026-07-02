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
import {
  ALL_STABLECOINS,
  onchainMint,
  onchainTokenProgram,
  type TokenProgramKind,
} from "../data/currencies";
import { BALANCE_REFETCH_DELAY_MS } from "../data/timings";
import { GET_MULTIPLE_ACCOUNTS_BATCH_SIZE } from "../env";
import { useAppEvent } from "../events";
import { getErrorMessage } from "../guards";
import { parseTokenAccountAmount } from "../validate";

// One shared map per page load, keyed by mint string. Semantics:
//   undefined → not yet fetched / disconnected
//   null      → no associated token account
//   0n        → ATA exists but balance is zero
//   > 0n      → balance in atomic units
type MaybeBalance = bigint | null | undefined;

const balances = new Map<string, bigint | null>();
let fetchedForOwner: string | null = null;
// Tracks the wallet currently visible to consumers. Updated by the effect
// below on connect/disconnect/wallet-swap so the swapSucceeded delayed
// refetch can drop if the wallet has changed since the timer was scheduled
// (otherwise the stale-owner write would land into the cache for whoever's
// connected now).
let activeOwner: string | null = null;
let lastFetchError: string | null = null;
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
// ALL_STABLECOINS so we can re-index parsed accounts back to mints. The ATA is
// derived from the on-chain mint (the mock demo mint on localnet, the real
// mint on mainnet) so localnet balances resolve — but results stay keyed by
// the real mint, which is what the rest of the app looks balances up by.
async function deriveAtas(owner: Address): Promise<Address[]> {
  return Promise.all(
    ALL_STABLECOINS.map(async (s) => {
      const [ata] = await findAssociatedTokenPda({
        owner,
        mint: address(onchainMint(s.mint)),
        tokenProgram: PROGRAM_FOR_KIND[onchainTokenProgram(s.mint)],
      });
      return ata;
    }),
  );
}

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
    // Chunked because many RPC providers cap getMultipleAccounts at a
    // small array size (e.g. PublicNode rejects >10 with 403). Batch size
    // of 0 = no chunking (single call).
    const size = GET_MULTIPLE_ACCOUNTS_BATCH_SIZE || atas.length;
    const chunks: Address[][] = [];
    for (let i = 0; i < atas.length; i += size) {
      chunks.push(atas.slice(i, i + size));
    }
    try {
      const responses = await Promise.all(
        chunks.map((chunk) =>
          rpc
            .getMultipleAccounts(chunk, {
              encoding: "jsonParsed",
              commitment: "confirmed",
            })
            .send(),
        ),
      );
      if (my !== requestCounter) return;
      lastFetchError = null;
      const accounts = responses.flatMap((r) => r.value);
      if (accounts.length !== ALL_STABLECOINS.length) {
        // Index-alignment guard: the chunked RPC results must come back in
        // the same order and count as the input ATAs, or balances would be
        // assigned to the wrong mints. Surface this distinctly so the cause
        // can be traced.
        throw new Error(
          `RPC returned ${accounts.length} accounts for ${ALL_STABLECOINS.length} ATAs — chunked getMultipleAccounts is out of alignment`,
        );
      }
      accounts.forEach((account, i) => {
        // Length-guarded above (accounts.length === ALL_STABLECOINS.length).
        const stable = ALL_STABLECOINS[i];
        if (!stable) return;
        const mint = stable.mint;
        if (!account) {
          balances.set(mint, null);
          return;
        }
        // jsonParsed gives us `data.parsed.info.tokenAmount.amount` as a
        // stringified bigint when the RPC recognized this as a token account.
        // If the shape is something else (raw binary, malformed parsed),
        // treat as zero rather than crashing the whole batch, but log so
        // the misclassification is visible.
        const amount = parseTokenAccountAmount(account.data);
        if (amount === null) {
          console.warn(
            `Balance fetch: RPC returned an account for ${mint} that wasn't a recognizable jsonParsed token account — falling back to 0n`,
          );
          balances.set(mint, 0n);
        } else {
          balances.set(mint, amount);
        }
      });
      fetchedForOwner = ownerStr;
      bump();
    } catch (e) {
      if (my !== requestCounter) return;
      const msg = getErrorMessage(e);
      // PublicNode (and likely other Cloudflare-fronted RPCs) returns 403
      // with this exact substring when the params array exceeds their
      // per-call cap. Surface a concrete fix instead of a generic error.
      if (/blocked parameter: params\.0/i.test(msg)) {
        console.error(
          `Balance fetch failed: RPC rejected getMultipleAccounts batch of ${size} accounts. ` +
            `Lower NEXT_PUBLIC_GET_MULTIPLE_ACCOUNTS_BATCH_SIZE in your env. Raw: ${msg}`,
        );
      } else {
        console.error("Balance fetch failed:", e);
      }
      lastFetchError = msg;
      // Don't set fetchedForOwner — leaving isReady=false keeps callers
      // that hide on loading from displaying misleading zero balances.
      // Consumers that want to surface the failure should read `error`.
      bump();
    }
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
  // Non-null when the most recent fetch failed; cleared on next success.
  error: string | null;
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
    activeOwner = ownerStr;
    if (!ownerStr) {
      if (fetchedForOwner !== null) {
        balances.clear();
        fetchedForOwner = null;
        lastFetchError = null;
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
  // Snapshot `ownerStr` and re-check `activeOwner` at fire time so a
  // wallet swap between the swap-success event and the delayed refetch
  // doesn't write the old owner's balances into the new owner's cache.
  useAppEvent("swapSucceeded", () => {
    const owner = ownerStr;
    if (!owner) return;
    void fetchBalances(client.runtime.rpc, owner, { force: true });
    window.setTimeout(() => {
      if (activeOwner !== owner) return;
      void fetchBalances(client.runtime.rpc, owner, { force: true });
    }, BALANCE_REFETCH_DELAY_MS);
  });

  return {
    balanceFor: (mint: string): MaybeBalance =>
      ownerStr === null ? undefined : balances.get(mint),
    isReady: ownerStr !== null && fetchedForOwner === ownerStr,
    error: ownerStr === null ? null : lastFetchError,
    refresh: () => {
      if (ownerStr) void fetchBalances(client.runtime.rpc, ownerStr);
    },
  };
}
