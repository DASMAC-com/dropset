"use client";

import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useCallback, useRef, useState } from "react";
import {
  type DepositLeg,
  executeVaultDeposit,
  executeVaultWithdraw,
  readDepositorShares,
  type VaultRef,
  type VaultTxOutcome,
} from "../eclob/vaultTx";
import { emit } from "../events";
import { getErrorMessage } from "../guards";
import { waitForSwapConfirmation } from "../swap/confirm";
import { SwapError } from "../swap/types";

export type VaultTxStatus =
  | "idle"
  // Re-reading the market and sizing, before the wallet is asked.
  | "preparing"
  | "signing"
  | "confirming"
  | "success"
  // Submitted, but confirmation ran out of time without a verdict: it may
  // still land, so the form stays locked rather than invite a second deposit
  // or withdrawal. `signature` names the transaction to check.
  | "unconfirmed"
  | "error";

export type VaultTxRequest =
  | {
      kind: "deposit";
      leg: DepositLeg;
      amountIn: bigint;
      walletAtoms: { base: bigint; quote: bigint };
    }
  | { kind: "withdraw"; bps: number };

export type UseVaultTx = {
  status: VaultTxStatus;
  error: string | null;
  // The share balance read back from chain after the last confirmed
  // transaction; null when none has confirmed or the read-back failed.
  sharesAfter: bigint | null;
  // The submitted transaction behind an "unconfirmed" or reverted outcome,
  // for an explorer link; null otherwise.
  signature: string | null;
  // Resolves true once the transaction confirms, false on any failure (the
  // reason is in `error`) or when one is already in flight.
  execute: (request: VaultTxRequest) => Promise<boolean>;
  reset: () => void;
};

// Send one vault deposit or withdraw and follow it to confirmation, on the
// swap hooks' lifecycle: one transaction in flight at a time, confirmation
// polled through the same waitForSwapConfirmation, and a success event fired
// so the balances and the vaults table refresh.
export function useVaultTx(vault: VaultRef): UseVaultTx {
  const wallet = useWallet();
  const client = useSolanaClient();
  const [status, setStatus] = useState<VaultTxStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [sharesAfter, setSharesAfter] = useState<bigint | null>(null);
  const [signature, setSignature] = useState<string | null>(null);
  const inFlight = useRef(false);

  const reset = useCallback(() => {
    setStatus("idle");
    setError(null);
    setSignature(null);
  }, []);

  const execute = useCallback(
    async (request: VaultTxRequest): Promise<boolean> => {
      if (inFlight.current) return false;
      if (wallet.status !== "connected") {
        setError("Connect a wallet first.");
        setStatus("error");
        return false;
      }
      inFlight.current = true;
      setError(null);
      setSignature(null);
      setStatus("preparing");
      const rpc = client.runtime.rpc;
      const ctx = {
        vault,
        owner: wallet.session.account.address.toString(),
        walletSession: wallet.session,
        rpc,
        onSigning: () => setStatus("signing"),
      };
      try {
        const res: VaultTxOutcome =
          request.kind === "deposit"
            ? await executeVaultDeposit({ ...ctx, ...request })
            : await executeVaultWithdraw({ ...ctx, ...request });
        setStatus("confirming");
        await waitForSwapConfirmation(rpc, res.signature, {
          lastValidBlockHeight: res.lastValidBlockHeight,
          noun: request.kind === "deposit" ? "deposit" : "withdrawal",
        });
        emit("vaultTxSucceeded");
        // The resulting balance comes off the chain, never the preview: the
        // program realizes the leader's fee before minting, so the shares it
        // actually issued can differ from the sizing. A failed read-back
        // leaves the transaction confirmed; only the figure is withheld.
        try {
          setSharesAfter(await readDepositorShares(rpc, res.depositor));
        } catch {
          setSharesAfter(null);
        }
        setStatus("success");
        return true;
      } catch (e) {
        setError(getErrorMessage(e));
        const unconfirmed = e instanceof SwapError && e.kind === "unconfirmed";
        setSignature(
          e instanceof SwapError && e.signature ? e.signature.toString() : null,
        );
        setStatus(unconfirmed ? "unconfirmed" : "error");
        // It may have landed, so the balances and the table may be stale.
        if (unconfirmed) emit("vaultTxSucceeded");
        return false;
      } finally {
        inFlight.current = false;
      }
    },
    [wallet, client, vault],
  );

  return { status, error, sharesAfter, signature, execute, reset };
}
