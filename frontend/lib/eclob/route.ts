"use client";

import {
  findMarketPda,
  PRICE_INFINITY,
  PRICE_ZERO,
  type PriceBits,
  type SwapSide,
} from "@dropset/sdk";
import type { SolanaClientRuntime } from "@solana/client";
import { type Address, address, fetchEncodedAccount } from "@solana/kit";
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

// A resolved eCLOB route against a market that actually exists on the current
// cluster: the market PDA and its raw bytes (already fetched), the take side,
// the no-bound limit price, and the base/quote mints + token programs the swap
// instruction needs.
export type EclobRoute = {
  market: Address;
  marketData: Uint8Array;
  baseMint: Address;
  quoteMint: Address;
  baseTokenProgram: Address;
  quoteTokenProgram: Address;
  side: SwapSide;
  limitPriceBits: PriceBits;
};

// One candidate market orientation for a pair: the pair mapped onto a
// base/quote market plus the take side a from→to swap would imply.
type Candidate = {
  baseMint: Address;
  quoteMint: Address;
  baseTokenProgram: Address;
  quoteTokenProgram: Address;
  side: SwapSide;
};

// Resolve the eCLOB route for a from→to pair by finding whichever market
// orientation actually exists on-chain. A Dropset market is a PDA of
// `[baseMint, quoteMint]`, and either token of the pair could be the base
// (the swap direction can flip), so both orientations are tried — the side
// follows from the one that exists:
//   - market(base=from, quote=to): the taker spends the base → a sell.
//   - market(base=to,  quote=from): the taker spends the quote → a buy.
// Returns null when neither orientation has a market (no eCLOB for this pair
// on this cluster), or on an unknown mint / same-token pair.
export async function resolveEclobRoute(
  rpc: Rpc,
  fromMint: string,
  toMint: string,
): Promise<EclobRoute | null> {
  if (!fromMint || !toMint || fromMint === toMint) return null;
  if (!stablecoinByMint(fromMint) || !stablecoinByMint(toMint)) return null;

  // Resolve against the on-chain mints for this cluster — mock demo mints on
  // localnet, real mints on mainnet. The market PDA, its account, and the swap
  // accounts are all keyed on what actually exists on-chain, while the caller
  // still passes the real (display) mints.
  const fromAddr = address(onchainMint(fromMint));
  const toAddr = address(onchainMint(toMint));
  const fromProgram = PROGRAM_FOR_KIND[onchainTokenProgram(fromMint)];
  const toProgram = PROGRAM_FOR_KIND[onchainTokenProgram(toMint)];

  const candidates: Candidate[] = [
    {
      baseMint: fromAddr,
      quoteMint: toAddr,
      baseTokenProgram: fromProgram,
      quoteTokenProgram: toProgram,
      side: "sell",
    },
    {
      baseMint: toAddr,
      quoteMint: fromAddr,
      baseTokenProgram: toProgram,
      quoteTokenProgram: fromProgram,
      side: "buy",
    },
  ];

  for (const c of candidates) {
    const [market] = await findMarketPda({
      baseMint: c.baseMint,
      quoteMint: c.quoteMint,
    });
    const marketData = await fetchMarketData(rpc, market);
    if (!marketData) continue;
    return {
      market,
      marketData,
      baseMint: c.baseMint,
      quoteMint: c.quoteMint,
      baseTokenProgram: c.baseTokenProgram,
      quoteTokenProgram: c.quoteTokenProgram,
      side: c.side,
      limitPriceBits: c.side === "buy" ? PRICE_INFINITY : PRICE_ZERO,
    };
  }
  return null;
}

// Fetch a market account's raw bytes (discriminator included — pass verbatim
// to simulateSwap), or null if the account doesn't exist. Uses the SDK's
// account-fetch primitive, which decodes the base64 payload for us.
export async function fetchMarketData(
  rpc: Rpc,
  market: Address,
): Promise<Uint8Array | null> {
  const account = await fetchEncodedAccount(rpc, market, {
    commitment: "confirmed",
  });
  return account.exists ? new Uint8Array(account.data) : null;
}
