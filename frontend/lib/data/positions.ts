import positionsData from "./positions.json";

// Outside-depositor positions, one per (owner, vault). This module is the seam
// between the UI and the position data source: today it parses a committed
// mock fixture (positions.json), but every consumer reads through the accessor
// below, so swapping in a real indexer fetch (or an on-chain VaultDepositor
// account read) later is a one-file change. Mirrors lib/data/vaults.ts.
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
  // Quote-denominated principal (the entrance amount), reduced pro-rata on
  // withdraw: `Σ (quote_in + base_in × entry_ref)`.
  netDeposits: number;
  // Shares-weighted average reference price (quote per base) across deposits.
  entryRefPrice: number;
  // Shares-weighted average value-per-share at entry; the basis for the
  // FX-neutral "yield since open".
  entryVps: number;
  // Slot of the first deposit.
  openedAtSlot: number;
};

// The mock depositor whose positions the preview surfaces. A connected wallet
// is treated as this owner so the seeded positions are visible without a real
// indexer; the live accessor will key off the connected wallet's pubkey.
export const MOCK_OWNER = "MockHolderBootStrapXk9PqVtZ7rWmYhJ2nGcQsLe4d";

const POSITIONS = (positionsData as { positions: VaultPosition[] }).positions;

// The caller's position in a vault, or null if they hold none. At most one row
// can match — the VaultDepositor PDA is soulbound (one per owner+vault), not a
// transferable NFT.
export const userPosition = (
  owner: string,
  vaultPubkey: string,
): VaultPosition | null =>
  POSITIONS.find((p) => p.owner === owner && p.vaultPubkey === vaultPubkey) ??
  null;
