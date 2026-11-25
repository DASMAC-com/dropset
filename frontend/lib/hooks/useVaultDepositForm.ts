"use client";

import { useWallet } from "@solana/react-hooks";
import { useState } from "react";
import { stablecoinMint } from "@/lib/data/currencies";
import type { VaultPosition } from "@/lib/data/positions";
import type { Vault, VaultMarket } from "@/lib/data/vaults";
import type { DepositLeg } from "@/lib/eclob/vaultTx";
import { useAllBalances } from "@/lib/hooks/useAllBalances";
import type { VaultTxRequest } from "@/lib/hooks/useVaultTx";
import {
  type DepositForm,
  depositForm,
  fillOtherLeg,
  legAtoms,
} from "@/lib/vaults/depositForm";

export type UseVaultDepositForm = {
  baseAmount: string;
  quoteAmount: string;
  // Which leg the user is driving (the target), or null with nothing entered.
  activeLeg: DepositLeg | null;
  // Set one leg's input; the other follows it pro-rata.
  setLeg: (leg: DepositLeg, value: string) => void;
  // Clear both legs, as after a confirmed deposit.
  reset: () => void;
  // The connected wallet's atoms per leg (0 until they load or with no
  // wallet), for the legs' Max.
  walletAtoms: { base: bigint; quote: bigint };
  // The submit button's label.
  label: string;
  // The deposit the entered basket sends, or null while it can't be sent.
  request: () => VaultTxRequest | null;
} & DepositForm;

// The vault dialog's deposit form: the two leg inputs, which one is driven,
// and the wallet's balances, over the pure model in lib/vaults/depositForm.ts.
// Deposit amounts link pro-rata to the vault through the program's own
// single-leg sizing, so the derived leg is exactly what will be pulled, and
// each leg rounds to its own token decimals.
export function useVaultDepositForm(
  market: VaultMarket,
  vault: Vault,
  position: VaultPosition | null,
): UseVaultDepositForm {
  const [baseAmount, setBaseAmount] = useState("");
  const [quoteAmount, setQuoteAmount] = useState("");
  // Which leg the user is driving (the target). The other stays editable but is
  // flagged with an "Auto" badge to show it's derived pro-rata; editing it just
  // flips the target. The exact amount pulled is settled at transaction time.
  const [activeLeg, setActiveLeg] = useState<DepositLeg | null>(null);

  const wallet = useWallet();
  const owner =
    wallet.status === "connected"
      ? wallet.session.account.address.toString()
      : null;

  // Per-leg wallet balance: the connected wallet's token accounts (read as 0
  // until they load or with no wallet; the model's `wallet` is null until then,
  // so that 0 never reads as a shortfall).
  const baseMint = stablecoinMint(market.base);
  const quoteMint = stablecoinMint(market.quote);
  const balances = useAllBalances([baseMint, quoteMint]);
  const walletAtoms = {
    base: balances.balanceFor(baseMint) ?? 0n,
    quote: balances.balanceFor(quoteMint) ?? 0n,
  };

  const form = depositForm({
    market,
    vault,
    owner,
    baseAmount,
    quoteAmount,
    activeLeg,
    wallet: owner !== null && balances.isReady ? walletAtoms : null,
  });

  // The other leg follows the driven one: the exact final the program will
  // pull (blank while it can't be sized).
  const setLeg = (leg: DepositLeg, value: string) => {
    const [setOwn, setOther] =
      leg === "base"
        ? [setBaseAmount, setQuoteAmount]
        : [setQuoteAmount, setBaseAmount];
    setOwn(value);
    setActiveLeg(value.trim() ? leg : null);
    setOther(fillOtherLeg(market, vault, leg, value));
  };

  const reset = () => {
    setBaseAmount("");
    setQuoteAmount("");
    setActiveLeg(null);
  };

  const request = (): VaultTxRequest | null => {
    if (!form.validBasket || form.depositBlocked || form.floorBreached)
      return null;
    if (activeLeg === null) return null;
    return {
      kind: "deposit",
      leg: activeLeg,
      amountIn: legAtoms(
        market,
        activeLeg,
        activeLeg === "base" ? baseAmount : quoteAmount,
      ),
      walletAtoms,
    };
  };

  return {
    ...form,
    baseAmount,
    quoteAmount,
    activeLeg,
    setLeg,
    reset,
    walletAtoms,
    label: position ? "Top off" : "Open position",
    request,
  };
}
