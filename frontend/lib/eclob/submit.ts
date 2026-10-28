"use client";

import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import {
  appendTransactionMessageInstructions,
  compileTransaction,
  createTransactionMessage,
  getBase64EncodedWireTransaction,
  type Instruction,
  pipe,
  type SendableTransaction,
  setTransactionMessageFeePayer,
  setTransactionMessageLifetimeUsingBlockhash,
  type Transaction,
  type TransactionSigner,
} from "@solana/kit";
import { getErrorMessage } from "../guards";
import { CANCEL_PATTERN, type Submitted, SwapError } from "../swap/types";

type Rpc = SolanaClientRuntime["rpc"];

// Fail before any RPC work when the wallet can do neither half of the hand-off.
export function assertWalletCanSign(walletSession: WalletSession): void {
  if (!walletSession.signTransaction && !walletSession.sendTransaction) {
    throw new SwapError(
      "Connected wallet can't sign or send transactions",
      "wallet",
    );
  }
}

// Compile `instructions` into a fee-payer-only v0 transaction and hand it to
// the wallet to sign and submit. Shared by the swap and the vault deposit /
// withdraw paths, so all three reach the chain the same way.
//
// No compute-budget instruction, deliberately: the wallet owns the budget.
// A wallet that adds a priority fee (Phantom does, at signing) skips any
// transaction that already carries a `SetComputeUnitLimit`, so setting one
// here would cost the signer that fee. Nor is one needed: with none, the
// runtime grants 200k CU per non-builtin instruction, shared across the
// transaction, so a two-ATA-create-plus-one-program-call transaction gets
// 600k. Less what the two ATA creates use, that covers every take the SDK's
// `compute_budget` docs measure — the costliest, a dust walk over a fully
// flipped book, included — and a vault deposit or withdraw, which walks no
// book, costs far less.
export async function submitInstructions({
  feePayer,
  instructions,
  walletSession,
  rpc,
}: {
  feePayer: TransactionSigner;
  instructions: readonly Instruction[];
  walletSession: WalletSession;
  rpc: Rpc;
}): Promise<Submitted> {
  const { value: latestBlockhash } = await rpc
    .getLatestBlockhash({ commitment: "finalized" })
    .send();
  const message = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayer(feePayer.address, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(latestBlockhash, m),
    (m) => appendTransactionMessageInstructions(instructions, m),
  );
  const tx = compileTransaction(message);
  const lifetime = {
    blockhash: latestBlockhash.blockhash,
    lastValidBlockHeight: latestBlockhash.lastValidBlockHeight,
  };

  try {
    // Cast via unknown: the compiled tx has an empty signature slot for the
    // fee payer (and carries a blockhash-lifetime brand), while the wallet's
    // parameter types want a fully-signed SendableTransaction. The wallet adds
    // the signature during signing, so the hand-off is runtime-safe.
    const unsigned = tx as unknown as SendableTransaction & Transaction;
    if (walletSession.signTransaction) {
      // Preferred path: the wallet only *signs*; we submit to our own RPC —
      // the same cluster we read the market from (localnet under `make demo`,
      // mainnet in normal mode). Submission therefore never depends on the
      // wallet's selected network, so a localnet transaction works without
      // switching the wallet (e.g. Phantom) off mainnet. The blockhash was
      // fetched from this same RPC, so it's valid on the node we submit to.
      const signed = await walletSession.signTransaction(unsigned);
      const signature = await rpc
        .sendTransaction(getBase64EncodedWireTransaction(signed), {
          encoding: "base64",
          preflightCommitment: "finalized",
        })
        .send();
      return { signature, lifetime };
    }
    if (walletSession.sendTransaction) {
      // Fallback for a wallet that can only sign-and-submit in one step: it
      // submits over its own network (fine on mainnet; a localnet demo needs
      // the wallet pointed at the local RPC in that case).
      const signature = await walletSession.sendTransaction(unsigned, {
        commitment: "finalized",
      });
      return { signature, lifetime };
    }
    throw new SwapError("Connected wallet can't sign", "wallet");
  } catch (e) {
    if (e instanceof SwapError) throw e;
    const msg = getErrorMessage(e);
    const cancelled = CANCEL_PATTERN.test(msg);
    throw new SwapError(
      cancelled ? "Cancelled in wallet" : msg,
      cancelled ? "rejected" : "wallet",
    );
  }
}
