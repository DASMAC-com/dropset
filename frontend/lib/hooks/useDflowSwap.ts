"use client";

import { executeDflowSwap } from "../dflow/dflowSwap";
import { percentToBps } from "../format/percent";
import type { Slippage } from "../store";
import type { SwapOutcome, UseSwap } from "../swap/types";
import { type SwapRoute, useSwapExecution } from "./useSwapExecution";

const slippageBpsParam = (slip: Slippage): string =>
  slip.mode === "auto" ? "auto" : percentToBps(slip.percent).toString();

// The aggregator reverts a failed swap in the ordinary way, so a confirmed
// transaction is a fill at the figures the order returned: no `settle`.
const DFLOW_ROUTE: SwapRoute<SwapOutcome> = {
  submit: ({ slippage, ...req }) =>
    executeDflowSwap({ ...req, slippageBps: slippageBpsParam(slippage) }),
};

// Swaps through the DFlow aggregator: GET /order, then the wallet's
// `sendTransaction`.
export function useDflowSwap(): UseSwap {
  return useSwapExecution(DFLOW_ROUTE);
}
