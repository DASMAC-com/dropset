"use client";

import {
  findMarketPda,
  getMarketHeaderDecoder,
  getMarketHeaderSize,
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
  // The leg the taker *receives* — base on a buy, quote on a sell. Derived
  // here, beside the `side` it follows from, rather than re-deduced at each
  // use: the platform fee is paid in this mint, and picking the wrong one
  // yields an ATA the program's `create_idempotent` CPI rejects. Note these
  // are the on-chain mints (mock demo mints on localnet), which is what the
  // fee destination must be derived against.
  outputMint: Address;
  outputTokenProgram: Address;
  // This market's on-chain ceiling on a declared `platform_fee_bps`. The
  // program rejects any swap above it, so callers clamp to this rather than
  // sending the configured rate blind — see `platformFeeBpsFor`.
  maxPlatformFeeBps: number;
};

// The platform fee this route may actually declare: the configured rate,
// clamped to the market's own ceiling.
//
// Clamping rather than failing, because the two outcomes are not symmetric. If
// an operator configures a rate above some market's ceiling, charging that
// market's maximum earns slightly less than intended; refusing the swap breaks
// trading on that pair outright and surfaces to the user as a broken quote
// (the simulator returns an all-zero quote for an over-ceiling rate, which the
// UI would report as "no liquidity" — a misleading diagnosis of what is really
// a config/ceiling mismatch). Under-charging is the safe direction for a knob
// that only sets our own revenue.
export function platformFeeBpsFor(
  route: EclobRoute,
  configuredBps: number,
): number {
  return Math.min(configuredBps, route.maxPlatformFeeBps);
}

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
      outputMint: c.side === "buy" ? c.baseMint : c.quoteMint,
      outputTokenProgram:
        c.side === "buy" ? c.baseTokenProgram : c.quoteTokenProgram,
      // Header-only decode: the ceiling is one scalar, and this runs on the
      // quote timer, so decoding the whole slab tail here would be waste.
      // (The generated `decodeMarketHeader` wants a fetched `EncodedAccount`;
      // we already hold the raw bytes, so slice and decode directly.)
      maxPlatformFeeBps: getMarketHeaderDecoder().decode(
        marketData.subarray(0, getMarketHeaderSize()),
      ).maxPlatformFee,
    };
  }
  return null;
}

// Fetch a market account's raw bytes (discriminator included — pass verbatim
// to simulateSwap), or null if the account doesn't exist. Uses the SDK's
// account-fetch primitive, which decodes the base64 payload for us. Internal:
// callers go through resolveEclobRoute, which returns the bytes it fetched.
async function fetchMarketData(
  rpc: Rpc,
  market: Address,
): Promise<Uint8Array | null> {
  const account = await fetchEncodedAccount(rpc, market, {
    commitment: "confirmed",
  });
  return account.exists ? new Uint8Array(account.data) : null;
}
