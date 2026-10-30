"use client";

import {
  ECLOB_DEFAULT_SLIPPAGE_BPS,
  type EclobSwapOutcome,
  executeEclobSwap,
} from "../eclob/eclobSwap";
import { percentToBps } from "../format/percent";
import type { Slippage } from "../store";
import { readRealizedFill } from "../swap/realizedFill";
import type { UseSwap } from "../swap/types";
import { type SwapRoute, useSwapExecution } from "./useSwapExecution";

const resolveSlippageBps = (slip: Slippage): number =>
  slip.mode === "auto"
    ? ECLOB_DEFAULT_SLIPPAGE_BPS
    : percentToBps(slip.percent);

const ECLOB_ROUTE: SwapRoute<EclobSwapOutcome> = {
  submit: ({ slippage, ...req }) =>
    executeEclobSwap({ ...req, slippageBps: resolveSlippageBps(slippage) }),

  // Confirmation only proves the transaction didn't revert, and on our own
  // program that is not the same as the swap having happened: a fill below
  // `minOut` soft-reverts, moves nothing, and still returns Ok. So settle
  // the quoted figures against the receipt before reporting anything. A
  // null reading is "couldn't tell" rather than "didn't fill" — an
  // unreadable receipt keeps the quoted figures rather than announcing a
  // swap that did not happen.
  settle: async (rpc, res) => {
    const realized = await readRealizedFill(rpc, res.signature, res.settlement);
    // An unreadable receipt is reported as a success at the quoted figures —
    // which is exactly the pre-fix behavior, so it is worth stating why it
    // is still the right direction rather than a hole in the fix.
    //
    // The two errors are not symmetric. Announcing "no funds were swapped"
    // about a swap that did happen invites the user to swap again, at their
    // own expense. Announcing a success about a swap that did not happen is
    // self-correcting: the balance refetch fired after settling shows the
    // unchanged balance moments later. So when the receipt cannot be read,
    // this fails in the direction that cannot cost the user money — and
    // `no-fill` is asserted only when the chain actually said so.
    return {
      outcome: {
        signature: res.signature,
        lifetime: res.lifetime,
        inAmount: realized?.amounts?.inAmount ?? res.inAmount,
        outAmount: realized?.amounts?.outAmount ?? res.outAmount,
      },
      filled: realized === null || realized.filled,
    };
  },
};

// The eCLOB-only counterpart to useDflowSwap: same status/result/error surface
// (so SwapPanel can pick either by route mode), but the swap is built and
// simulated directly against our market via executeEclobSwap — no DFlow.
export function useEclobSwap(): UseSwap {
  return useSwapExecution(ECLOB_ROUTE);
}
