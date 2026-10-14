// cspell:word atas
"use client";

import { type Address, address } from "@solana/kit";
import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { findAssociatedTokenPda } from "@solana-program/token";
import { useEffect, useSyncExternalStore } from "react";
import {
  onchainMint,
  onchainTokenProgram,
  PROGRAM_FOR_KIND,
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

type Rpc = ReturnType<typeof useSolanaClient>["runtime"]["rpc"];

// Balances are fetched per mint, only for the mints a mounted consumer shows:
// the swap rows ask for the pair, the picker's cells for their own rows. A
// cold load pays for two accounts, not the whole catalogue; the rest load
// when a picker opens.
const balances = new Map<string, bigint | null>();
// Mints fetched, and mints in flight, for `cacheOwner`.
const fetched = new Set<string>();
const pending = new Set<string>();
let cacheOwner: string | null = null;
// Tracks the wallet currently visible to consumers. Updated by the effect
// below on connect/disconnect/wallet-swap so the swapSucceeded delayed
// refetch can drop if the wallet has changed since the timer was scheduled
// (otherwise the stale-owner write would land into the cache for whoever's
// connected now).
let activeOwner: string | null = null;
let lastFetchError: string | null = null;
// Per-mint request sequence, so an older fetch landing after a newer one
// (a mount-time fetch overtaken by a swap-success force refetch) can't write
// a stale balance.
let sequence = 0;
const latestFor = new Map<string, number>();
// Mints requested since the last flush. Consumers mounting in one commit each
// request their own mints from their effects; flushing on a microtask merges
// them into one chunked fetch instead of a call per consumer.
const queued = new Set<string>();
let queuedFor: { rpc: Rpc; owner: string } | null = null;
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

// Wipe the cache when the owner changes, so a reconnect to a different
// wallet doesn't briefly show the prior owner's balances. A fetch still in
// flight for the prior owner drops its write (it checks `cacheOwner`).
function resetFor(owner: string | null): void {
  if (cacheOwner === owner) return;
  const hadData = fetched.size > 0 || pending.size > 0;
  balances.clear();
  fetched.clear();
  pending.clear();
  queued.clear();
  latestFor.clear();
  lastFetchError = null;
  cacheOwner = owner;
  if (hadData) bump();
}

// Queue `mints` for `owner`. Without `force`, a mint already fetched or in
// flight is skipped; `force` fetches it again regardless (after a swap lands).
function request(
  rpc: Rpc,
  owner: string,
  mints: readonly string[],
  force = false,
): void {
  resetFor(owner);
  for (const mint of mints) {
    if (force || (!fetched.has(mint) && !pending.has(mint))) queued.add(mint);
  }
  if (queued.size === 0) return;
  const scheduled = queuedFor !== null;
  queuedFor = { rpc, owner };
  if (!scheduled) queueMicrotask(flush);
}

function flush(): void {
  const target = queuedFor;
  queuedFor = null;
  const mints = [...queued];
  queued.clear();
  if (!target || mints.length === 0 || cacheOwner !== target.owner) return;
  const requestIds = mints.map((mint) => {
    pending.add(mint);
    const seq = ++sequence;
    latestFor.set(mint, seq);
    return seq;
  });
  void fetchMints(target.rpc, target.owner, mints, requestIds);
}

// The ATA is derived from the on-chain mint (the mock demo mint on localnet,
// the real mint on mainnet) so localnet balances resolve — but results stay
// keyed by the real mint, which is what the rest of the app looks balances up
// by.
const deriveAta = async (owner: Address, mint: string): Promise<Address> => {
  const [ata] = await findAssociatedTokenPda({
    owner,
    mint: address(onchainMint(mint)),
    tokenProgram: PROGRAM_FOR_KIND[onchainTokenProgram(mint)],
  });
  return ata;
};

async function fetchMints(
  rpc: Rpc,
  ownerStr: string,
  mints: string[],
  requestIds: number[],
): Promise<void> {
  // A write lands only if the owner is unchanged and no newer request for
  // that mint was issued meanwhile.
  const current = (i: number): boolean => {
    const mint = mints[i];
    return (
      cacheOwner === ownerStr &&
      mint !== undefined &&
      latestFor.get(mint) === requestIds[i]
    );
  };
  const owner = address(ownerStr);
  const atas = await Promise.all(mints.map((m) => deriveAta(owner, m)));
  // Chunked because many RPC providers cap getMultipleAccounts at a small
  // array size (e.g. PublicNode rejects >10 with 403). Batch size of 0 = no
  // chunking (single call).
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
    const accounts = responses.flatMap((r) => r.value);
    if (accounts.length !== mints.length) {
      // Index-alignment guard: the chunked RPC results must come back in
      // the same order and count as the input ATAs, or balances would be
      // assigned to the wrong mints. Surface this distinctly so the cause
      // can be traced.
      throw new Error(
        `RPC returned ${accounts.length} accounts for ${mints.length} ATAs — chunked getMultipleAccounts is out of alignment`,
      );
    }
    if (cacheOwner !== ownerStr) return;
    lastFetchError = null;
    accounts.forEach((account, i) => {
      const mint = mints[i];
      if (mint === undefined || !current(i)) return;
      pending.delete(mint);
      fetched.add(mint);
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
    bump();
  } catch (e) {
    if (cacheOwner !== ownerStr) return;
    // Release the mints so a later request (remount, refresh) retries them.
    mints.forEach((mint, i) => {
      if (current(i)) pending.delete(mint);
    });
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
    // Leaving the mints unfetched keeps isReady=false, so callers that hide
    // on loading don't display misleading zero balances. Consumers that
    // want to surface the failure should read `error`.
    lastFetchError = msg;
    bump();
  }
}

export type UseAllBalances = {
  // null → no ATA, 0n → empty account, > 0n → balance, undefined → loading
  balanceFor: (mint: string) => MaybeBalance;
  // True once every mint in this consumer's scope has loaded.
  isReady: boolean;
  // Non-null when the most recent fetch failed; cleared on next success.
  error: string | null;
  refresh: () => void;
};

// Wallet balances from the shared cache, fetching the mints in `scope` (the
// ones this consumer shows) on connect or wallet swap.
export function useAllBalances(scope: readonly string[]): UseAllBalances {
  const wallet = useWallet();
  const client = useSolanaClient();
  const ownerStr =
    wallet.status === "connected"
      ? wallet.session.account.address.toString()
      : null;

  // Re-render any consumer when the cache changes.
  useSyncExternalStore(subscribe, getVersion, getVersion);

  // Keyed on the joined scope rather than the array, so a caller passing an
  // inline array doesn't re-run this on every render.
  const scopeKey = scope.join(",");
  useEffect(() => {
    activeOwner = ownerStr;
    if (!ownerStr) {
      resetFor(null);
      return;
    }
    request(client.runtime.rpc, ownerStr, scopeKey ? scopeKey.split(",") : []);
  }, [ownerStr, client, scopeKey]);

  // After a swap lands, re-fetch everything loaded so far — once now and once
  // ~1.5 s later to absorb RPC propagation lag between confirmation status
  // and account state. Every mounted consumer hears the event; the queue
  // merges their requests into one fetch. Snapshot `ownerStr` and re-check
  // `activeOwner` at fire time so a wallet swap between the swap-success
  // event and the delayed refetch doesn't write the old owner's balances into
  // the new owner's cache.
  useAppEvent("swapSucceeded", () => {
    const owner = ownerStr;
    if (!owner) return;
    const refetch = () => {
      if (activeOwner !== owner) return;
      request(client.runtime.rpc, owner, [...fetched, ...scope], true);
    };
    refetch();
    window.setTimeout(refetch, BALANCE_REFETCH_DELAY_MS);
  });

  return {
    balanceFor: (mint: string): MaybeBalance =>
      ownerStr === null ? undefined : balances.get(mint),
    isReady:
      ownerStr !== null &&
      cacheOwner === ownerStr &&
      scope.every((mint) => fetched.has(mint)),
    error: ownerStr === null ? null : lastFetchError,
    refresh: () => {
      if (ownerStr) request(client.runtime.rpc, ownerStr, scope, true);
    },
  };
}
