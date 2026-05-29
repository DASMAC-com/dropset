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
import { stablecoinByMint, type TokenProgramKind } from "../data/currencies";
import { PLATFORM_FEE } from "../env";
import { getErrorMessage } from "../guards";

// A "fee vault" is the platform-fee wallet's associated token account for a
// given output mint. DFlow's /order endpoint rejects a request whose
// `feeAccount` doesn't exist on-chain, and even an *uncollected* declared fee
// eats into slippage budget — so the fee may only be advertised and charged
// for mints whose ATA was pre-created (via `pnpm setup-fee-atas`).
//
// Existence is resolved lazily, per output mint, the first time that mint is
// actually relevant (selected into the swap's to-side, or about to be swapped
// into) — never eagerly for the whole currency list. That keeps anonymous
// page loads off the RPC entirely and turns each check into a single
// getAccountInfo. Results are cached for the page's lifetime; a reload picks
// up vaults created after the check. The fee wallet is constant per
// deployment, so the cache needs no per-wallet keying.

type Rpc = SolanaClientRuntime["rpc"];

const PROGRAM_FOR_KIND: Record<TokenProgramKind, Address> = {
  classic: TOKEN_PROGRAM_ADDRESS,
  token2022: TOKEN_2022_PROGRAM_ADDRESS,
};

// mint → { ata, exists }. A present entry means the mint has been checked;
// `exists` reflects whether the ATA was found on-chain, and `ata` is cached so
// the swap path doesn't re-derive it. A failed check leaves no entry, so the
// next trigger (a re-selection or a swap attempt) retries — bounded to one
// getAccountInfo per attempt.
type VaultInfo = { ata: Address; exists: boolean };
const vaults = new Map<string, VaultInfo>();
// Per-mint dedupe so a re-render, the swap path, and rapid to-token toggles
// don't fan out into parallel checks for the same mint.
const inFlightByMint = new Map<string, Promise<void>>();

// Tiny external store so UI consumers re-render once a vault check lands.
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

// Resolve (and cache) whether the fee vault for a single output mint exists.
// No-op when no fee is configured, the mint isn't a known stablecoin, or the
// mint was already checked. Deduped per mint via inFlightByMint.
function checkFeeVault(rpc: Rpc, outputMint: string): Promise<void> {
  if (!PLATFORM_FEE) return Promise.resolve();
  if (vaults.has(outputMint)) return Promise.resolve();
  const existing = inFlightByMint.get(outputMint);
  if (existing) return existing;
  const stable = stablecoinByMint(outputMint);
  if (!stable) return Promise.resolve();

  const owner = PLATFORM_FEE.wallet;
  const promise = (async () => {
    const [ata] = await findAssociatedTokenPda({
      owner,
      mint: address(outputMint),
      tokenProgram: PROGRAM_FOR_KIND[stable.tokenProgram],
    });
    const { value } = await rpc
      .getAccountInfo(ata, { encoding: "base64", commitment: "confirmed" })
      .send();
    vaults.set(outputMint, { ata, exists: value != null });
    bump();
  })()
    .catch((e) => {
      // Leave the mint uncached so a later trigger retries. Callers treat an
      // unchecked mint as "no vault", which fails safe: we skip the fee rather
      // than risk a /order rejection.
      console.error(
        `Fee vault check failed for ${outputMint}:`,
        getErrorMessage(e),
      );
    })
    .finally(() => {
      if (inFlightByMint.get(outputMint) === promise) {
        inFlightByMint.delete(outputMint);
      }
    });
  inFlightByMint.set(outputMint, promise);
  return promise;
}

// Swap-path entry point: ensure the output mint's vault has been checked, then
// return its fee ATA only if the vault exists. Returns null when no fee is
// configured, the mint isn't a known stablecoin, or its vault is missing — any
// of which means the fee must not be declared to DFlow. Normally a cache hit,
// since the UI already checked the mint when it was selected into the to-side.
export async function resolveFeeAccount(
  rpc: Rpc,
  outputMint: string,
): Promise<Address | null> {
  if (!PLATFORM_FEE) return null;
  if (!stablecoinByMint(outputMint)) return null;
  await checkFeeVault(rpc, outputMint);
  const info = vaults.get(outputMint);
  return info?.exists ? info.ata : null;
}

// UI hook: lazily checks the given output mint's fee vault and re-renders once
// the result lands. The check only fires while `enabled` is true — callers
// pass the swap's actionable state so we never hit the RPC for a swap that
// can't happen (no wallet connected, insufficient input balance, etc.).
// Returns true → vault exists (fee will be charged), false → missing,
// undefined → not yet checked (or no fee configured), so the swap panel only
// advertises the fee once it's confirmed chargeable.
export function useFeeVaultExists(
  outputMint: string,
  enabled: boolean,
): boolean | undefined {
  const client = useSolanaClient();
  useSyncExternalStore(subscribe, getVersion, getVersion);

  useEffect(() => {
    if (enabled && PLATFORM_FEE && outputMint) {
      void checkFeeVault(client.runtime.rpc, outputMint);
    }
  }, [client, outputMint, enabled]);

  if (!PLATFORM_FEE) return false;
  return vaults.get(outputMint)?.exists;
}
