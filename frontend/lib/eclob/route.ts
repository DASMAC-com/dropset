"use client";

import {
  type EclobRoute,
  platformFeeBpsFor,
  resolveEclobRoute as sdkResolveEclobRoute,
} from "@dropset/sdk";
import type { SolanaClientRuntime } from "@solana/client";
import { type Address, address } from "@solana/kit";
import { TOKEN_PROGRAM_ADDRESS } from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";
import {
  onchainMint,
  onchainTokenProgram,
  stablecoinByMint,
  type TokenProgramKind,
} from "../data/currencies";

type Rpc = SolanaClientRuntime["rpc"];

export const PROGRAM_FOR_KIND: Record<TokenProgramKind, Address> = {
  classic: TOKEN_PROGRAM_ADDRESS,
  token2022: TOKEN_2022_PROGRAM_ADDRESS,
};

// The route shape and the market-ceiling clamp both live in the SDK now — the
// route because the router owns resolution, and the clamp because it reads a
// field of that route. Re-exported here so app-side callers keep one import
// site for "the eCLOB route and what it permits".
export type { EclobRoute };
export { platformFeeBpsFor };

// Resolve the eCLOB route for a from→to pair. The market-orientation search
// itself lives in the SDK (`@dropset/sdk` → resolveEclobRoute); what stays here
// is the app-specific part the SDK can't know: the supported-stablecoin gate,
// and the last-moment substitution of the mock demo mints on localnet.
//
// The SDK takes mints in *on-chain* terms, while callers pass the real
// (display) mints — so `onchainMint` / `onchainTokenProgram` translate at this
// boundary. Both are the identity on mainnet. Passing the token programs
// explicitly also spares the SDK two mint-account reads per call, which
// matters on the quote timer.
//
// Returns null when either token isn't a supported stablecoin, the pair is
// degenerate, or no market exists for it on this cluster.
export async function resolveEclobRoute(
  rpc: Rpc,
  fromMint: string,
  toMint: string,
): Promise<EclobRoute | null> {
  if (!fromMint || !toMint || fromMint === toMint) return null;
  if (!stablecoinByMint(fromMint) || !stablecoinByMint(toMint)) return null;

  return sdkResolveEclobRoute(
    rpc,
    {
      inputMint: address(onchainMint(fromMint)),
      outputMint: address(onchainMint(toMint)),
      inputTokenProgram: PROGRAM_FOR_KIND[onchainTokenProgram(fromMint)],
      outputTokenProgram: PROGRAM_FOR_KIND[onchainTokenProgram(toMint)],
    },
    { commitment: "confirmed" },
  );
}
