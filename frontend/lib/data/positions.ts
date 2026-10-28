// cspell:ignore soulbound

// An outside depositor's position, one per (owner, vault), read from the
// owner's VaultDepositor account (lib/data/chainVaults.ts → chainPosition).
//
// See docs/architecture.md → "Depositor positions and cost basis": the
// VaultDepositor account stores the depositor's claim (`shares`) and cost
// basis (`netDeposits`, `entryRefPrice`, `entryVps`, `openedAt`). It's a
// soulbound PDA seeded by ("vault_depositor", vault, owner), so there is at
// most one position per (owner, vault).
export type VaultPosition = {
  vaultPubkey: string;
  owner: string;
  // Pro-rata claim on the vault; `shares / vault.totalShares` is the fraction
  // of reserves owned.
  shares: number;
  // Quote-denominated principal (the cost basis of the shares STILL held),
  // reduced pro-rata on withdraw: `Σ (quote_in + base_in × entry_ref)`.
  netDeposits: number;
  // Monotonic lifetime contributions: grows on every deposit, never reduced on
  // withdraw, so it's the stable denominator for an all-time return % and
  // doubles as "total deposited". Always ≥ netDeposits; equal until the first
  // withdrawal. See docs/architecture.md → "Depositor positions and cost basis".
  grossDeposited: number;
  // Shares-weighted average reference price (quote per base) across deposits.
  entryRefPrice: number;
  // Shares-weighted average value-per-share at entry; the basis for the
  // FX-neutral "yield since open".
  entryVps: number;
  // Slot of the first deposit.
  openedAtSlot: number;
  // Lifetime realized-PnL accumulators (signed, quote-denominated), grown on
  // each withdrawal as the released slice's gains are booked. 0 for a position
  // that's never been withdrawn from; they only reset on a full exit, when the
  // VaultDepositor account closes. All-time PnL = realized + unrealized. See
  // docs/architecture.md → "Depositor positions and cost basis" → All-time PnL.
  realizedPnl: number;
  realizedYield: number;
  realizedPrice: number;
};
