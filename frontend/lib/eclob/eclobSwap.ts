"use client";

import {
  DROPSET_PROGRAM_ADDRESS,
  getSwapInstructionAsync,
  initSimulator,
  simulateSwap,
} from "@dropset/sdk";
import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import {
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
import {
  CANCEL_PATTERN,
  DflowSwapError,
  type DflowSwapResult,
} from "../dflow/dflowSwap";
import { getErrorMessage } from "../guards";
import { resolveEclobRoute } from "./route";

type Rpc = SolanaClientRuntime["rpc"];

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
// less the (clamped) slippage tolerance. Rounds down (integer division), so
// the actual floor is never looser than requested.
const applySlippage = (out: bigint, bps: number): bigint => {
  const clamped = BigInt(
    Math.min(Math.max(Math.trunc(bps), 0), MAX_SLIPPAGE_BPS),
  );
  return (out * (BPS_DENOMINATOR - clamped)) / BPS_DENOMINATOR;
};

// Execute an eCLOB swap end-to-end, the direct-SDK counterpart to
// executeDflowSwap:
//   1. Resolve the route (market PDA, side, mints, token programs).
//   2. Read the market bytes + current slot and re-simulate — the quote is
//      re-derived here (not trusted from the UI) so `minOut` reflects the
//      book at submit time, mirroring how the DFlow path re-fetches /order.
//   3. Build the swap instruction (idempotently creating the taker's ATAs
//      first, so a first-time output token doesn't fail the transfer), compile
//      to a fee-payer-only transaction, and hand it to the wallet to sign +
//      submit.
export async function executeEclobSwap(
  input: EclobSwapInput,
): Promise<DflowSwapResult> {
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
    throw new DflowSwapError(
      "Connected wallet can't sign or send transactions",
      "wallet",
    );
  }

  const route = await resolveEclobRoute(rpc, inputMint, outputMint);
  if (!route) {
    throw new DflowSwapError("No Dropset market for this pair", "api");
  }

  const slot = await rpc.getSlot({ commitment: "confirmed" }).send();

  await initSimulator();
  const quote = simulateSwap(
    route.marketData,
    route.side,
    atomicAmount,
    route.limitPriceBits,
    Number(slot),
  );
  if (quote.outAmount === 0n) {
    throw new DflowSwapError("No liquidity for this size", "api");
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
    side: route.side === "buy" ? 0 : 1,
    amountIn: atomicAmount,
    limitPriceBits: route.limitPriceBits,
    minOut,
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
      throw new DflowSwapError("Connected wallet can't sign", "wallet");
    }
  } catch (e) {
    const msg = getErrorMessage(e);
    const cancelled = CANCEL_PATTERN.test(msg);
    throw new DflowSwapError(
      cancelled ? "Cancelled in wallet" : msg,
      cancelled ? "rejected" : "wallet",
    );
  }

  return { signature, inAmount: quote.inAmount, outAmount: quote.outAmount };
}
