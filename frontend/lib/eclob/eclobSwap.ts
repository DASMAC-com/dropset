"use client";

import {
  getSwapInstructionAsync,
  initSimulator,
  simulateSwap,
} from "@dropset/sdk";
import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import { type Address, address, createNoopSigner } from "@solana/kit";
import {
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstructionAsync,
} from "@solana-program/token";
import { PLATFORM_FEE } from "../env";
import type { SettlementRef } from "../swap/realizedFill";
import { SwapError, type SwapOutcome } from "../swap/types";
import { readGateClocks } from "./chainClock";
import { type EclobRoute, platformFeeBpsFor, resolveEclobRoute } from "./route";
import { assertWalletCanSign, submitInstructions } from "./submit";

type Rpc = SolanaClientRuntime["rpc"];

/**
 * The quoted outcome, plus what the caller needs to read back what the
 * transaction *actually* moved once it confirms.
 *
 * The two amounts here come from the pre-flight simulation and are a
 * prediction, not a receipt: a fill below `minOut` soft-reverts and moves
 * nothing while still returning a successful transaction, so a caller must
 * settle these against `readRealizedFill` before reporting them to a user.
 * `settlement` is carried out of here rather than re-derived by the caller
 * because the on-chain mints are a property of the resolved route (mock demo
 * mints on localnet), which does not outlive this call.
 */
export type EclobSwapOutcome = SwapOutcome & {
  settlement: SettlementRef;
};

/**
 * Which mints the taker spends and receives on this route, for `owner`.
 *
 * A pure function of the route, and exported so the base/quote mapping can be
 * pinned: it mirrors the SDK's own derivation of `outputMint` (base on a buy,
 * quote on a sell) and a transposition here would be silent and user-visible.
 * Both mints still belong to the taker, so the balance reader would find both,
 * measure each in the wrong direction, and — after its coherence check —
 * withhold the amounts on a swap that really filled.
 */
export const settlementFor = (
  route: EclobRoute,
  owner: Address,
): SettlementRef => ({
  owner,
  inputMint: route.side === "sell" ? route.baseMint : route.quoteMint,
  outputMint: route.outputMint,
});

export type EclobSwapInput = {
  inputMint: string;
  outputMint: string;
  // Input amount in base units (already scaled by the from-mint's decimals).
  atomicAmount: bigint;
  // Slippage tolerance in basis points — applied to the freshly simulated
  // output to compute the on-chain `minOut` soft-revert floor.
  slippageBps: number;
  // The output the panel showed the user when they pressed Swap. The swap is
  // refused when the fresh simulation falls more than `slippageBps` below it,
  // so the figure the user agreed to bounds what is signed.
  quotedOutAmount: bigint;
  userPublicKey: string;
  walletSession: WalletSession;
  rpc: Rpc;
};

const BPS_DENOMINATOR = 10_000n;
// The store's slippage input is uncapped (it can exceed 100%), but a bps of
// 10000+ would zero or negate minOut — disabling the on-chain floor, or
// overflowing the u64 instruction arg. Cap at 99.99% so minOut stays positive
// and the swap always carries a real floor.
const MAX_SLIPPAGE_BPS = 9_999;

// The output floor below which the swap soft-reverts: the simulated output
// less the (clamped) slippage tolerance.
//
// Integer division truncates, which *lowers* the floor — so the floor this
// returns is loose by at most one atom rather than tight. An output of 1001 at
// 50 bps has an exact floor of 995.995 and this yields 995, so a fill at 995
// clears a floor the taker sized at 995.995. One atom is far below any
// meaningful slippage, and rounding the other way would reject fills sitting
// exactly on a floor the taker did ask for, so the direction is deliberate.
// It is spelled out because the looseness is not self-evident from the
// expression, and a reader who assumed the safe direction was guaranteed
// could build on it.
//
// A non-finite `bps` is clamped before it reaches `BigInt`: `Math.trunc(NaN)`
// is `NaN`, and `Math.min`/`Math.max` propagate it, so without the guard
// `BigInt(NaN)` throws a RangeError out of a swap that has already been
// quoted. It falls back to zero — the tightest floor, not this module's idea
// of a sensible tolerance. Zero is a legitimate setting ("exact or nothing")
// rather than an error value, so the conflation is deliberate: a slippage
// that arrived as NaN is an upstream bug, and on a money path the safe
// failure is one that risks no funds. The swap then soft-reverts on any
// adverse move, which the caller now reports as a swap that did not happen.
//
// Exported only so the rounding direction and the non-finite guard can be
// pinned directly — both are one-line properties whose failure is silent, and
// neither is reachable through `executeEclobSwap` without a wallet and an RPC.
export const applySlippage = (out: bigint, bps: number): bigint => {
  const finite = Number.isFinite(bps) ? Math.trunc(bps) : 0;
  const clamped = BigInt(Math.min(Math.max(finite, 0), MAX_SLIPPAGE_BPS));
  return (out * (BPS_DENOMINATOR - clamped)) / BPS_DENOMINATOR;
};

// Execute an eCLOB swap end-to-end, the direct-SDK counterpart to
// executeDflowSwap:
//   1. Resolve the route (market PDA, side, mints, token programs).
//   2. Read the market bytes and re-simulate at the current clocks — the quote is
//      re-derived here (not trusted from the UI) so `minOut` reflects the
//      book at submit time, mirroring how the DFlow path re-fetches /order.
//   3. Build the swap instruction (idempotently creating the taker's ATAs
//      first, so a first-time output token doesn't fail the transfer) and
//      hand it to submitInstructions to compile, sign and submit.
export async function executeEclobSwap(
  input: EclobSwapInput,
): Promise<EclobSwapOutcome> {
  const {
    inputMint,
    outputMint,
    atomicAmount,
    slippageBps,
    quotedOutAmount,
    userPublicKey,
    walletSession,
    rpc,
  } = input;

  assertWalletCanSign(walletSession);

  const route = await resolveEclobRoute(rpc, inputMint, outputMint);
  if (!route) {
    throw new SwapError("No Dropset market for this pair", "api");
  }

  // Declare the same configured rate the DFlow route declares, paid to the
  // same wallet DFlow's `feeAccount` names, so revenue from both routes lands
  // in one place and neither route is the cheap one to route around.
  //
  // "Same rate", not "same fee in every case" — the two still diverge where
  // their preconditions differ: DFlow drops the fee entirely for an output
  // mint whose fee ATA doesn't exist, and the eCLOB route clamps to the
  // market's on-chain ceiling (below). On such a pair, toggling the route does
  // change what the user pays; the fee row reports whichever rate actually
  // applied, so the panel never overstates it.
  //
  // Two differences from the DFlow path, both because the fee is settled by
  // our own program here rather than by a third party:
  //   * No existence check on the destination. DFlow's /order rejects a
  //     missing `feeAccount` (the reason lib/dflow/feeVault.ts caches that
  //     answer); our `swap` creates it via an ATA-program `create_idempotent`
  //     CPI instead, with the taker funding the rent.
  //   * Derived against the route's *on-chain* output mint and token program,
  //     not the display mint. On localnet those differ (mock demo mints), and
  //     a display-mint derivation would produce an address the program's own
  //     ATA derivation rejects.
  const platformFeeBps = PLATFORM_FEE
    ? platformFeeBpsFor(route, PLATFORM_FEE.bps)
    : 0;
  const [feeAta] = PLATFORM_FEE
    ? await findAssociatedTokenPda({
        owner: PLATFORM_FEE.wallet,
        mint: route.outputMint,
        tokenProgram: route.outputTokenProgram,
      })
    : [undefined];

  await initSimulator();
  // Level expiry is dual-domain, so the re-simulation needs both clocks: the
  // chain's slot and the wall clock each quote's datums are measured from. A
  // level rests only inside both of its deadlines, and the engine measures the
  // second against cluster time — so the device clock is checked against the
  // chain (lib/eclob/chainClock.ts) rather than trusted, and the slot, read at
  // `finalized` and so already behind head, is nudged forward by the
  // slot-domain margin. Both corrections point the same way, because this is
  // the sizing that sets `minOut` below: gating against levels the engine has
  // already dropped sizes a floor the fill cannot reach, and the swap
  // soft-reverts — moving no funds, but still spending the network fee and the
  // rent for a first-time output ATA (created by the separate instructions
  // below, which the swap's own rollback does not reach). Read after the
  // simulator is up, so neither clock ages across its first instantiation.
  const { nowSlot, nowUnix } = await readGateClocks(rpc);
  // Quote *with* the fee so `minOut` is a floor on what actually lands in the
  // taker's account. Passing 0 here and netting the fee off afterwards would
  // round differently from the engine (both fees truncate, in a fixed order)
  // and leave minOut a few atoms adrift from the real fill.
  const quote = simulateSwap(
    route.marketData,
    route.side,
    atomicAmount,
    route.limitPriceBits,
    nowSlot,
    nowUnix,
    platformFeeBps,
  );
  if (quote.outAmount === 0n) {
    throw new SwapError("No liquidity for this size", "api");
  }
  // A partial fill spends less than the user asked to swap at the book's
  // deepest levels, which the panel's figure does not describe; the best
  // route's `classifyEclobQuote` excludes partials for the same reason.
  if (quote.inAmount !== atomicAmount) {
    throw new SwapError(
      "The book can no longer fill this size — reduce the amount",
      "api",
    );
  }
  if (quote.outAmount < applySlippage(quotedOutAmount, slippageBps)) {
    throw new SwapError(
      "The price moved past your slippage since the quote — review it and try again",
      "api",
    );
  }
  const minOut = applySlippage(quote.outAmount, slippageBps);

  const taker = createNoopSigner(address(userPublicKey));
  const [takerBaseAta] = await findAssociatedTokenPda({
    owner: taker.address,
    mint: route.baseMint,
    tokenProgram: route.baseTokenProgram,
  });
  const [takerQuoteAta] = await findAssociatedTokenPda({
    owner: taker.address,
    mint: route.quoteMint,
    tokenProgram: route.quoteTokenProgram,
  });
  const [marketBaseTreasury] = await findAssociatedTokenPda({
    owner: route.market,
    mint: route.baseMint,
    tokenProgram: route.baseTokenProgram,
  });
  const [marketQuoteTreasury] = await findAssociatedTokenPda({
    owner: route.market,
    mint: route.quoteMint,
    tokenProgram: route.quoteTokenProgram,
  });

  // Idempotently create both taker ATAs. The input ATA already exists (it's
  // funded), so that create is a no-op; the output ATA may not exist yet on a
  // first-time swap into that token, and the transfer would fail without it.
  const [createBaseAta, createQuoteAta] = await Promise.all([
    getCreateAssociatedTokenIdempotentInstructionAsync({
      payer: taker,
      owner: taker.address,
      mint: route.baseMint,
      tokenProgram: route.baseTokenProgram,
    }),
    getCreateAssociatedTokenIdempotentInstructionAsync({
      payer: taker,
      owner: taker.address,
      mint: route.quoteMint,
      tokenProgram: route.quoteTokenProgram,
    }),
  ]);

  // The route carries the program its market was found under, so the
  // instruction, its `program` account and the PDAs the builder derives all
  // agree with the market.
  const { programAddress } = route;
  const swapIx = await getSwapInstructionAsync(
    {
      taker,
      market: route.market,
      baseMint: route.baseMint,
      quoteMint: route.quoteMint,
      baseTokenProgram: route.baseTokenProgram,
      quoteTokenProgram: route.quoteTokenProgram,
      takerBaseAta,
      takerQuoteAta,
      marketBaseTreasury,
      marketQuoteTreasury,
      program: programAddress,
      // Omitted (undefined) when no fee is declared, which the generated client
      // encodes as the program-id sentinel Anchor reads back as `None`. The
      // program rejects a non-zero rate with either slot absent, so both are
      // gated on the same condition.
      platformFeeAuthority:
        platformFeeBps > 0 ? PLATFORM_FEE?.wallet : undefined,
      platformFeeAta: platformFeeBps > 0 ? feeAta : undefined,
      side: route.side === "buy" ? 0 : 1,
      amountIn: atomicAmount,
      limitPriceBits: route.limitPriceBits,
      minOut,
      platformFeeBps,
    },
    { programAddress },
  );

  const submitted = await submitInstructions({
    feePayer: taker,
    instructions: [createBaseAta, createQuoteAta, swapIx],
    walletSession,
    rpc,
  });

  return {
    ...submitted,
    inAmount: quote.inAmount,
    outAmount: quote.outAmount,
    settlement: settlementFor(route, taker.address),
  };
}
