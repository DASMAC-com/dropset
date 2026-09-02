"use client";

import {
  DROPSET_PROGRAM_ADDRESS,
  getSwapInstructionAsync,
  initSimulator,
  simulateSwap,
} from "@dropset/sdk";
import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import {
  type Address,
  address,
  appendTransactionMessageInstructions,
  compileTransaction,
  createNoopSigner,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  pipe,
  type SendableTransaction,
  type Signature,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  type Transaction,
} from "@solana/kit";
import {
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstructionAsync,
} from "@solana-program/token";
import { PLATFORM_FEE } from "../env";
import { getErrorMessage } from "../guards";
import type { SettlementRef } from "../swap/realizedFill";
import { CANCEL_PATTERN, SwapError, type SwapOutcome } from "../swap/types";
import { gateNowSlot, gateNowUnix, syncChainClock } from "./chainClock";
import { type EclobRoute, platformFeeBpsFor, resolveEclobRoute } from "./route";

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
//      first, so a first-time output token doesn't fail the transfer), compile
//      to a fee-payer-only transaction, and hand it to the wallet to sign +
//      submit.
export async function executeEclobSwap(
  input: EclobSwapInput,
): Promise<EclobSwapOutcome> {
  const {
    inputMint,
    outputMint,
    atomicAmount,
    slippageBps,
    userPublicKey,
    walletSession,
    rpc,
  } = input;

  if (!walletSession.signTransaction && !walletSession.sendTransaction) {
    throw new SwapError(
      "Connected wallet can't sign or send transactions",
      "wallet",
    );
  }

  const route = await resolveEclobRoute(rpc, inputMint, outputMint);
  if (!route) {
    throw new SwapError("No Dropset market for this pair", "api");
  }

  // Level expiry is dual-domain, so the re-simulation needs both clocks: the
  // chain's slot and the wall clock each quote's datums are measured from. A
  // level rests only inside both of its deadlines, and the engine measures the
  // second against cluster time — so the device clock is checked against the
  // chain here (lib/eclob/chainClock.ts) rather than trusted, and the slot,
  // read at `confirmed` and so already behind head, is nudged forward by the
  // slot-domain margin. Both corrections point the same way, because this is
  // the sizing that sets `minOut` below: gating against levels the engine has
  // already dropped sizes a floor the fill cannot reach, and the swap
  // soft-reverts — moving no funds, but still spending the network fee and the
  // rent for a first-time output ATA (created by the separate instructions
  // below, which the swap's own rollback does not reach).
  //
  // `syncChainClock` gets the RAW slot: it asks the node for that block's
  // production time, and a slot nudged past head has no block.
  const slot = await rpc.getSlot({ commitment: "confirmed" }).send();
  await syncChainClock(rpc, slot);

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
  // Quote *with* the fee so `minOut` is a floor on what actually lands in the
  // taker's account. Passing 0 here and netting the fee off afterwards would
  // round differently from the engine (both fees truncate, in a fixed order)
  // and leave minOut a few atoms adrift from the real fill.
  const quote = simulateSwap(
    route.marketData,
    route.side,
    atomicAmount,
    route.limitPriceBits,
    gateNowSlot(slot),
    gateNowUnix(),
    platformFeeBps,
  );
  if (quote.outAmount === 0n) {
    throw new SwapError("No liquidity for this size", "api");
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

  const swapIx = await getSwapInstructionAsync({
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
    program: DROPSET_PROGRAM_ADDRESS,
    // Omitted (undefined) when no fee is declared, which the generated client
    // encodes as the program-id sentinel Anchor reads back as `None`. The
    // program rejects a non-zero rate with either slot absent, so both are
    // gated on the same condition.
    platformFeeAuthority: platformFeeBps > 0 ? PLATFORM_FEE?.wallet : undefined,
    platformFeeAta: platformFeeBps > 0 ? feeAta : undefined,
    side: route.side === "buy" ? 0 : 1,
    amountIn: atomicAmount,
    limitPriceBits: route.limitPriceBits,
    minOut,
    platformFeeBps,
  });

  const { value: latestBlockhash } = await rpc
    .getLatestBlockhash({ commitment: "confirmed" })
    .send();
  const message = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayer(taker.address, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(latestBlockhash, m),
    (m) =>
      appendTransactionMessageInstructions(
        [createBaseAta, createQuoteAta, swapIx],
        m,
      ),
  );
  const tx = compileTransaction(message);

  let signature: Signature;
  try {
    // Cast via unknown: the compiled tx has an empty signature slot for the
    // taker (and carries a blockhash-lifetime brand), while the wallet's
    // parameter types want a fully-signed SendableTransaction. The wallet adds
    // the signature during signing, so the hand-off is runtime-safe.
    const unsigned = tx as unknown as SendableTransaction & Transaction;
    if (walletSession.signTransaction) {
      // Preferred path: the wallet only *signs*; we submit to our own RPC —
      // the same cluster we quoted against (localnet under `make demo`,
      // mainnet in normal mode). Submission therefore never depends on the
      // wallet's selected network, so a localnet swap works without switching
      // the wallet (e.g. Phantom) off mainnet. The blockhash was fetched from
      // this same RPC, so it's valid on the node we submit to.
      const signed = await walletSession.signTransaction(unsigned);
      signature = await rpc
        .sendTransaction(getBase64EncodedWireTransaction(signed), {
          encoding: "base64",
          preflightCommitment: "confirmed",
        })
        .send();
    } else if (walletSession.sendTransaction) {
      // Fallback for a wallet that can only sign-and-submit in one step: it
      // submits over its own network (fine on mainnet; a localnet demo needs
      // the wallet pointed at the local RPC in that case).
      signature = await walletSession.sendTransaction(
        tx as unknown as Transaction & SendableTransaction,
        { commitment: "confirmed" },
      );
    } else {
      throw new SwapError("Connected wallet can't sign", "wallet");
    }
  } catch (e) {
    const msg = getErrorMessage(e);
    const cancelled = CANCEL_PATTERN.test(msg);
    throw new SwapError(
      cancelled ? "Cancelled in wallet" : msg,
      cancelled ? "rejected" : "wallet",
    );
  }

  return {
    signature,
    inAmount: quote.inAmount,
    outAmount: quote.outAmount,
    settlement: settlementFor(route, taker.address),
  };
}
