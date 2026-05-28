// cspell:word atas
"use client";

import type { SolanaClientRuntime } from "@solana/client";
import { type Address, address } from "@solana/kit";
import { useSolanaClient } from "@solana/react-hooks";
import {
  findAssociatedTokenPda,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";
import { useEffect, useSyncExternalStore } from "react";
import {
  ALL_STABLECOINS,
  stablecoinByMint,
  type TokenProgramKind,
} from "../data/currencies";
import { GET_MULTIPLE_ACCOUNTS_BATCH_SIZE, PLATFORM_FEE } from "../env";
import { getErrorMessage } from "../guards";

// "Fee vaults" are the platform-fee wallet's associated token accounts (one
// per output mint). DFlow's /order endpoint rejects a request whose
// `feeAccount` doesn't exist on-chain, and even an *uncollected* declared fee
// eats into slippage budget — so the fee may only be advertised and charged
// for mints whose ATA was pre-created (via `pnpm setup-fee-atas`). This module
// resolves which vaults exist once per page load and serves the answer to both
// the swap path (to decide whether to charge) and the UI (to decide whether to
// report). The fee wallet is constant per deployment, so a single fetch keyed
// by mint is enough; a page reload picks up vaults created after load.

type Rpc = SolanaClientRuntime["rpc"];

const PROGRAM_FOR_KIND: Record<TokenProgramKind, Address> = {
  classic: TOKEN_PROGRAM_ADDRESS,
  token2022: TOKEN_2022_PROGRAM_ADDRESS,
};

// mint → { ata, exists }. Absent = not yet resolved. `exists` reflects whether
// the ATA was found on-chain; `ata` is cached so the swap path doesn't
// re-derive it.
type VaultInfo = { ata: Address; exists: boolean };
const vaults = new Map<string, VaultInfo>();
let resolved = false;
let inFlight: Promise<void> | null = null;

// Tiny external store so UI consumers re-render once vault existence lands.
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

// Resolve every stablecoin's fee ATA under the platform-fee wallet and check
// which exist on-chain. Deduped: concurrent callers attach to the in-flight
// fetch, and once resolved the cache is reused for the rest of the page load.
function loadFeeVaults(rpc: Rpc): Promise<void> {
  if (!PLATFORM_FEE) return Promise.resolve();
  if (resolved) return Promise.resolve();
  if (inFlight) return inFlight;

  const owner = PLATFORM_FEE.wallet;
  inFlight = (async () => {
    const entries = await Promise.all(
      ALL_STABLECOINS.map(async (s) => {
        const [ata] = await findAssociatedTokenPda({
          owner,
          mint: address(s.mint),
          tokenProgram: PROGRAM_FOR_KIND[s.tokenProgram],
        });
        return { mint: s.mint, ata };
      }),
    );
    // Chunked because many RPC providers cap getMultipleAccounts at a small
    // array size (mirrors useAllBalances). Batch size of 0 = single call.
    const size = GET_MULTIPLE_ACCOUNTS_BATCH_SIZE || entries.length;
    const chunks: (typeof entries)[] = [];
    for (let i = 0; i < entries.length; i += size) {
      chunks.push(entries.slice(i, i + size));
    }
    const responses = await Promise.all(
      chunks.map((chunk) =>
        rpc
          .getMultipleAccounts(
            chunk.map((e) => e.ata),
            { encoding: "base64", commitment: "confirmed" },
          )
          .send(),
      ),
    );
    const flatEntries = chunks.flat();
    const accounts = responses.flatMap((r) => r.value);
    flatEntries.forEach((entry, i) => {
      vaults.set(entry.mint, { ata: entry.ata, exists: accounts[i] != null });
    });
    resolved = true;
    bump();
  })()
    .catch((e) => {
      // Leave `resolved` false so a later attempt can retry. An unknown vault
      // is treated as "doesn't exist" by callers, which fails safe: we skip
      // the fee rather than risk a /order rejection.
      console.error("Fee vault existence check failed:", getErrorMessage(e));
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

// Swap-path entry point: ensure vaults are resolved, then return the fee ATA
// for `outputMint` only if it exists on-chain. Returns null when no fee is
// configured, the mint isn't a known stablecoin, or its vault is missing —
// any of which means the fee must not be declared to DFlow.
export async function resolveFeeAccount(
  rpc: Rpc,
  outputMint: string,
): Promise<Address | null> {
  if (!PLATFORM_FEE) return null;
  if (!stablecoinByMint(outputMint)) return null;
  await loadFeeVaults(rpc);
  const info = vaults.get(outputMint);
  return info?.exists ? info.ata : null;
}

export type UseFeeVaults = {
  // true → vault exists (fee will be charged), false → missing, undefined →
  // not yet resolved. Always false when no platform fee is configured.
  vaultExists: (mint: string) => boolean | undefined;
};

// UI hook: kicks the existence fetch on mount and re-renders consumers once it
// lands, so the swap panel can advertise the fee only for mints that will
// actually be charged.
export function useFeeVaults(): UseFeeVaults {
  const client = useSolanaClient();
  useSyncExternalStore(subscribe, getVersion, getVersion);

  useEffect(() => {
    if (PLATFORM_FEE) void loadFeeVaults(client.runtime.rpc);
  }, [client]);

  return {
    vaultExists: (mint: string): boolean | undefined => {
      if (!PLATFORM_FEE) return false;
      return vaults.get(mint)?.exists;
    },
  };
}
