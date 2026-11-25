import { stablecoinDecimals } from "@/lib/data/currencies";
import {
  contributeClosedReason,
  leaderFloorFraction,
  maxOutsideDepositValue,
  type Vault,
  type VaultMarket,
  vaultRefPrice,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import {
  type DepositLeg,
  type DepositSizing,
  depositSizingError,
  sizeDeposit,
  VAULT_SLIPPAGE_BPS,
} from "@/lib/eclob/vaultTx";
import { formatBaseAmount, parseAmountToBase } from "@/lib/format/balance";

// The vault dialog's deposit form as a pure model: what the entered amounts
// size to, whether the wallet covers them, whether the vault takes them, and
// what they are worth. The dialog's hook (lib/hooks/useVaultDepositForm.ts)
// holds the inputs; everything here is derived from them, so the sizing and
// validation rules are tested without rendering.

type Symbols = Pick<VaultMarket, "base" | "quote">;

// One leg's amount in the token's atoms, or the reason the program would
// reject it; null for an empty or zero amount (nothing to size).
export type LegSizing = { sizing: DepositSizing } | { error: string } | null;

const legSymbol = (market: Symbols, leg: DepositLeg): string =>
  leg === "base" ? market.base : market.quote;

// The entered amount on `leg` in the token's atoms.
export const legAtoms = (
  market: Symbols,
  leg: DepositLeg,
  value: string,
): bigint =>
  parseAmountToBase(value, stablecoinDecimals(legSymbol(market, leg)));

// The deposit sized by the SDK on the leg being driven: the basket the
// program will pull and the shares it will mint, or why it would reject the
// amount.
export const sizeLeg = (
  market: Symbols,
  vault: Vault,
  leg: DepositLeg,
  value: string,
): LegSizing => {
  const amountIn = legAtoms(market, leg, value);
  if (amountIn === 0n) return null;
  try {
    return {
      sizing: sizeDeposit(vault.chain, leg, amountIn, VAULT_SLIPPAGE_BPS),
    };
  } catch (e) {
    return { error: depositSizingError(e, leg, market) };
  }
};

// Atoms → a whole-token input string at the token's full precision, the same
// shape DepositLeg's padToken gives a float.
const atomsToInput = (atoms: bigint, symbol: string): string => {
  const decimals = stablecoinDecimals(symbol);
  return formatBaseAmount(atoms, decimals, decimals);
};

// The other leg's input once `leg` is set to `value`: the exact final the
// program will pull, blank while the driven leg can't be sized.
export const fillOtherLeg = (
  market: Symbols,
  vault: Vault,
  leg: DepositLeg,
  value: string,
): string => {
  const sized = sizeLeg(market, vault, leg, value);
  if (!sized || !("sizing" in sized)) return "";
  return leg === "base"
    ? atomsToInput(sized.sizing.quoteIn, market.quote)
    : atomsToInput(sized.sizing.baseIn, market.base);
};

export type DepositFormInput = {
  market: Symbols;
  vault: Vault;
  // The connected wallet's address, or null with none.
  owner: string | null;
  baseAmount: string;
  quoteAmount: string;
  // Which leg the user is driving; null with nothing entered.
  activeLeg: DepositLeg | null;
  // The wallet's atoms per leg once its balances are known; null while they
  // load or with no wallet, so a not-yet-read 0 never reads as a shortfall.
  wallet: { base: bigint; quote: bigint } | null;
};

export type DepositForm = {
  // Each leg's display value in USD (the ≈ under its input).
  legs: { base: { usd: number }; quote: { usd: number } };
  // The driven leg's sizing, or null while it can't be sized.
  sizing: DepositSizing | null;
  // The sizing rejection or the wallet shortfall, worded for the user.
  error: string | null;
  // A sized basket the known wallet covers.
  validBasket: boolean;
  // Why the vault takes no deposit from this wallet, or null when it does.
  closedReason: string | null;
  depositBlocked: boolean;
  // The basket would dilute the leader below their min_leader_share floor.
  floorBreached: boolean;
  // That floor as a whole percent, for the warning.
  floorPct: number;
  // The basket's total USD value, display only.
  usd: number;
};

// A parsed leg amount, 0 for an empty or non-numeric input.
const legNumber = (value: string): number => {
  const n = Number.parseFloat(value);
  return Number.isFinite(n) ? n : 0;
};

export const depositForm = ({
  market,
  vault,
  owner,
  baseAmount,
  quoteAmount,
  activeLeg,
  wallet,
}: DepositFormInput): DepositForm => {
  const base = legNumber(baseAmount);
  const quote = legNumber(quoteAmount);
  // Quote tokens per base token; null for an empty vault (no ratio to hold).
  const ratio = vaultReserveRatio(vault);

  // The sizing of the driven leg, and whether the wallet covers the basket it
  // pulls. A one-sided vault pulls nothing on its empty leg, so a basket needs
  // only one positive leg.
  const sized =
    activeLeg === null
      ? null
      : sizeLeg(
          market,
          vault,
          activeLeg,
          activeLeg === "base" ? baseAmount : quoteAmount,
        );
  const sizing = sized && "sizing" in sized ? sized.sizing : null;
  // A basket is checked against the wallet only once its balances are known:
  // while they load, or with no wallet, there is no shortfall to report, just
  // nothing to submit yet.
  const balancesKnown = owner !== null && wallet !== null;
  const shortOf =
    sizing && wallet && balancesKnown
      ? sizing.baseIn > wallet.base
        ? market.base
        : sizing.quoteIn > wallet.quote
          ? market.quote
          : null
      : null;
  const error =
    (sized && "error" in sized ? sized.error : null) ??
    (shortOf ? `Not enough ${shortOf} in your wallet.` : null);
  const validBasket = sizing !== null && balancesKnown && shortOf === null;

  // The program's own gate and the surface's closed states, checked against
  // the connected wallet so a leader isn't offered a deposit into their own
  // vault; past them, a deposit still needs a wallet to come from.
  const closedReason =
    contributeClosedReason(market, vault, owner) ??
    (owner === null ? "Connect a wallet to deposit." : null);

  // Quote-denominated value of the entered basket (the base leg marked at the
  // vault's reserve ratio). Tested against the deposit cap below — the cap is
  // in the same quote/share units, so this stays quote-denominated.
  const depositValue = base * (ratio ?? 0) + quote;
  // USD value of the basket, for display only ("Total Deposit" + per-leg ≈).
  // Each leg's quote-denominated value × the quote token's USD price, so the
  // legs and the total share one basis and read as real USD even when the
  // quote leg isn't a USD stable (e.g. GYEN). The base leg is priced at the
  // vault's reference price, not the basket ratio, which measures holdings.
  const baseUsd = base * (vaultRefPrice(vault) ?? ratio ?? 0) * vault.quoteUsd;
  const quoteUsd = quote * vault.quoteUsd;
  // The largest deposit the vault can take before the new shares would dilute
  // the leader below their min_leader_share floor; null when the floor can't
  // bind. A basket past the cap would be rejected onchain, so it is blocked
  // and warned rather than let the user submit a doomed deposit.
  const depositCap = maxOutsideDepositValue(vault);
  const floorBreached =
    depositCap !== null && validBasket && depositValue > depositCap;

  return {
    legs: { base: { usd: baseUsd }, quote: { usd: quoteUsd } },
    sizing,
    error,
    validBasket,
    closedReason,
    depositBlocked: closedReason !== null,
    floorBreached,
    floorPct: Math.round(leaderFloorFraction(vault) * 100),
    usd: baseUsd + quoteUsd,
  };
};
