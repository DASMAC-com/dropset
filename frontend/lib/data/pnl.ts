import type { VaultPosition } from "./positions";
import type { Vault } from "./vaults";

// Display-only PnL decomposition for a depositor position, per
// docs/architecture.md → "Depositor positions and cost basis". The protocol
// stores cost basis on-chain, not PnL; a UI marks the position to a display
// price `refNow` (the live ReferencePrice, or an external FX feed) and splits
// the result into the FX-neutral yield (leader skill / spread capture) and the
// directional FX move on the base leg.
//
// For a position of `shares` in a vault holding B base / Q quote over
// totalShares, the current basket is base_out = shares · B / totalShares and
// quote_out = shares · Q / totalShares, and:
//
//   currentValue     = quote_out + base_out · refNow
//   valueAtEntryFx   = quote_out + base_out · entryRefPrice
//   yieldPnl         = valueAtEntryFx − netDeposits      (ex-FX: spread vs adverse selection)
//   fxPnl            = base_out · (refNow − entryRefPrice)
//   netPnl           = currentValue − netDeposits = yieldPnl + fxPnl
//
// `yieldPctSinceOpen` is the oracle-free pure-skill figure VPS_now / entryVps − 1.
export type PositionPnl = {
  currentValue: number;
  entranceAmount: number;
  yieldPnl: number;
  fxPnl: number;
  netPnl: number;
  yieldPctSinceOpen: number;
};

// The depositor's current claim on the vault's reserves: base_out / quote_out
// = shares · reserve / totalShares. Zero when the vault has no shares.
export const positionBasket = (
  position: VaultPosition,
  vault: Vault,
): { baseOut: number; quoteOut: number } => {
  const fraction =
    vault.totalShares > 0 ? position.shares / vault.totalShares : 0;
  return {
    baseOut: fraction * vault.baseReserve,
    quoteOut: fraction * vault.quoteReserve,
  };
};

// `refNow` is the display reference price (quote per base). Until a price feed
// exists callers pass vaultReserveRatio(vault) as the stand-in.
export const positionPnl = (
  position: VaultPosition,
  vault: Vault,
  refNow: number,
): PositionPnl => {
  const { baseOut, quoteOut } = positionBasket(position, vault);

  const currentValue = quoteOut + baseOut * refNow;
  const valueAtEntryFx = quoteOut + baseOut * position.entryRefPrice;
  const yieldPnl = valueAtEntryFx - position.netDeposits;
  const fxPnl = baseOut * (refNow - position.entryRefPrice);
  const netPnl = currentValue - position.netDeposits;
  const yieldPctSinceOpen =
    position.entryVps > 0 ? vault.vps / position.entryVps - 1 : 0;

  return {
    currentValue,
    entranceAmount: position.netDeposits,
    yieldPnl,
    fxPnl,
    netPnl,
    yieldPctSinceOpen,
  };
};

export type AllTimePnl = {
  allTimeYield: number;
  allTimeFx: number;
  allTimePnl: number;
};

// Lifetime PnL = realized (booked on past withdrawals, stored on the position)
// + unrealized (the current marked position). Split the same two ways. See
// docs/architecture.md → "Depositor positions and cost basis" → All-time PnL.
export const allTimePnl = (
  position: VaultPosition,
  vault: Vault,
  refNow: number,
): AllTimePnl => {
  const { yieldPnl, fxPnl, netPnl } = positionPnl(position, vault, refNow);
  return {
    allTimeYield: position.realizedYield + yieldPnl,
    allTimeFx: position.realizedFx + fxPnl,
    allTimePnl: position.realizedPnl + netPnl,
  };
};

export type WithdrawalPreview = {
  baseOut: number;
  quoteOut: number;
  value: number;
  realizedPnl: number;
  remainingValue: number;
};

// Preview redeeming a fraction `w` of the position. Shares are one fungible
// claim, so a withdrawal is always a pro-rata slice of the *whole* basket: it
// returns w of each leg and realizes w of the net PnL (you can't withdraw only
// the yield leg or only the FX leg). `fraction` is clamped to [0, 1]; at 1 the
// preview equals the full position and the VaultDepositor PDA would close.
export const withdrawalPreview = (
  position: VaultPosition,
  vault: Vault,
  refNow: number,
  fraction: number,
): WithdrawalPreview => {
  const w = Math.min(1, Math.max(0, fraction));
  const { baseOut, quoteOut } = positionBasket(position, vault);
  const { currentValue, netPnl } = positionPnl(position, vault, refNow);
  return {
    baseOut: w * baseOut,
    quoteOut: w * quoteOut,
    value: w * currentValue,
    realizedPnl: w * netPnl,
    remainingValue: (1 - w) * currentValue,
  };
};
