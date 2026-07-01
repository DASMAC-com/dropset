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
  stablecoinByMint,
  stablecoinMint,
  type TokenProgramKind,
} from "../data/currencies";

type Rpc = SolanaClientRuntime["rpc"];

export const PROGRAM_FOR_KIND: Record<TokenProgramKind, Address> = {
  classic: TOKEN_PROGRAM_ADDRESS,
  token2022: TOKEN_2022_PROGRAM_ADDRESS,
};

// A resolved eCLOB route: the market PDA for the pair, its base/quote mints
// and token programs, the take side, and the no-bound limit price for a market
// take. USDC is the universal quote, so a pair routes only when exactly one
// side is USDC (every Dropset market is <token>/USDC).
export type EclobRoute = {
  market: Address;
  baseMint: Address;
  quoteMint: Address;
  baseTokenProgram: Address;
  quoteTokenProgram: Address;
  side: SwapSide;
  limitPriceBits: PriceBits;
};

// Resolve the eCLOB route for a from→to mint pair, or null when there's no
// direct market (neither side is USDC, same token, or an unknown mint). Side
// is from the taker's view: spending USDC for the base is a buy; spending the
// base for USDC is a sell. A market take is unbounded, so the limit price is
// the per-side no-bound sentinel.
export async function resolveEclobRoute(
  fromMint: string,
  toMint: string,
): Promise<EclobRoute | null> {
  if (!fromMint || !toMint || fromMint === toMint) return null;
  const usdc = stablecoinMint("USDC");
  if (!usdc) return null;

  let baseMintStr: string;
  let side: SwapSide;
  if (fromMint === usdc) {
    baseMintStr = toMint;
    side = "buy";
  } else if (toMint === usdc) {
    baseMintStr = fromMint;
    side = "sell";
  } else {
    return null;
  }

  const base = stablecoinByMint(baseMintStr);
  const quote = stablecoinByMint(usdc);
  if (!base || !quote) return null;

  const baseMint = address(baseMintStr);
  const quoteMint = address(usdc);
  const [market] = await findMarketPda({ baseMint, quoteMint });
  return {
    market,
    baseMint,
    quoteMint,
    baseTokenProgram: PROGRAM_FOR_KIND[base.tokenProgram],
    quoteTokenProgram: PROGRAM_FOR_KIND[quote.tokenProgram],
    side,
    limitPriceBits: side === "buy" ? PRICE_INFINITY : PRICE_ZERO,
  };
}

// Fetch a market account's raw bytes (discriminator included — pass verbatim
// to simulateSwap), or null if the account doesn't exist (market not created
// on this cluster). Uses the SDK's account-fetch primitive, which decodes the
// base64 payload for us.
export async function fetchMarketData(
  rpc: Rpc,
  market: Address,
): Promise<Uint8Array | null> {
  const account = await fetchEncodedAccount(rpc, market, {
    commitment: "confirmed",
  });
  return account.exists ? new Uint8Array(account.data) : null;
}
