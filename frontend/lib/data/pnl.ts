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
