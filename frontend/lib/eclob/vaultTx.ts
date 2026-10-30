"use client";

import {
  BasketError,
  computeProRataSlice,
  decodeDropsetVaults,
  fetchMaybeVaultDepositorHeader,
  findVaultDepositorPda,
  getDepositInstructionAsync,
  getWithdrawInstructionAsync,
  initSimulator,
  realizePerfFee,
  singleLegBasket,
  type VaultSummary,
} from "@dropset/sdk";
import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import {
  type Address,
  address,
  createNoopSigner,
  type TransactionSigner,
} from "@solana/kit";
import {
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstructionAsync,
} from "@solana-program/token";
import { stablecoinMint } from "../data/currencies";
import {
  depositClosedReason,
  leaderFloorHolds,
  type Vault,
} from "../data/vaults";
import { type Submitted, SwapError } from "../swap/types";
import { applySlippage } from "./eclobSwap";
import { type EclobRoute, resolveEclobRoute } from "./route";
import { assertWalletCanSign, submitInstructions } from "./submit";

// The vault write path: size a deposit or withdraw with the SDK's own integer
// kernels (the TS mirror of what the program runs), then build, sign and send
// it the way the swap is sent. Sizing is pure and runs twice — live in the
// dialog against the table's vault state for the preview, then here against a
// fresh read of the market just before the transaction is built, since the
// table can be a poll period stale.
//
// The program realizes the leader's performance fee before sizing
// (`realize_in_place`), which mints shares to the leader and leaves the atoms
// untouched. A withdraw's slice shrinks by the minted fraction, and the fee
// accrues across every trade between deposits and withdraws, so it can
// outgrow any fixed tolerance: a withdraw is therefore sized against the
// realized share count, run through the SDK's mirror of the same kernel. A
// deposit is not: its atom amounts are exact either way, and only its
// `sharesOut` may come out slightly higher (each share then worth slightly
// less). The share balance shown after a transaction is read back from
// chain, never taken from the preview.

// The leader-floor check lives beside the table's "Full" state it must agree
// with; re-exported for the send path's callers.
export { leaderFloorHolds };

type Rpc = SolanaClientRuntime["rpc"];

const U64_MAX = (1n << 64n) - 1n;
const BPS_DENOMINATOR = 10_000n;

// The tolerance on the leg a deposit derives and on a withdraw's floors,
// covering the vault's mix moving (trades against its book, the fee they
// accrue) between the pre-build read and the transaction landing. A
// withdraw's floors are per leg, not on value: a fill that shifts the mix
// more than this fails the withdraw even though its value barely moved. The
// eCLOB swap's default, so the two paths carry one figure.
export const VAULT_SLIPPAGE_BPS = 50;

// The vault state sizing reads: the raw counts the program sizes against.
export type VaultAtoms = Pick<
  VaultSummary,
  "baseAtoms" | "quoteAtoms" | "totalShares"
>;

export type DepositLeg = "base" | "quote";

export type DepositSizing = {
  sharesOut: bigint;
  // The basket the program will pull, rounded up as it rounds.
  baseIn: bigint;
  quoteIn: bigint;
  // The instruction's bounds: the driving leg is capped at what was entered
  // (its final never exceeds it), the derived leg at its final plus the
  // slippage tolerance.
  maxBaseIn: bigint;
  maxQuoteIn: bigint;
};

// Size a single-leg deposit of `amountIn` atoms on `leg`, exactly as the
// program's `single_leg_basket` will. Throws `BasketError` where the program
// would reject it (an empty leg, too small for one share).
export function sizeDeposit(
  vault: VaultAtoms,
  leg: DepositLeg,
  amountIn: bigint,
  slippageBps: number,
): DepositSizing {
  const baseLeg = leg === "base" ? amountIn : 0n;
  const quoteLeg = leg === "quote" ? amountIn : 0n;
  const { sharesOut, baseInFinal, quoteInFinal } = singleLegBasket(
    vault.totalShares,
    vault.baseAtoms,
    vault.quoteAtoms,
    baseLeg,
    quoteLeg,
    U64_MAX,
    U64_MAX,
  );
  // The inverse of applySlippage: widen the derived leg's ceiling by the
  // tolerance, rounding up so the bound never lands below the final itself.
  const widen = (n: bigint): bigint => {
    const bps = BigInt(Math.max(0, Math.trunc(slippageBps)));
    const widened = n + (n * bps + BPS_DENOMINATOR - 1n) / BPS_DENOMINATOR;
    return widened > U64_MAX ? U64_MAX : widened;
  };
  return {
    sharesOut,
    baseIn: baseInFinal,
    quoteIn: quoteInFinal,
    maxBaseIn: leg === "base" ? amountIn : widen(baseInFinal),
    maxQuoteIn: leg === "quote" ? amountIn : widen(quoteInFinal),
  };
}

// A user-facing reason for a sizing rejection, naming the legs by symbol.
export function depositSizingError(
  e: unknown,
  leg: DepositLeg,
  symbols: { base: string; quote: string },
): string {
  const [entered, other] =
    leg === "base"
      ? [symbols.base, symbols.quote]
      : [symbols.quote, symbols.base];
  if (e instanceof BasketError) {
    if (e.kind === "EmptyLeg")
      return `This vault holds no ${entered}. Enter the ${other} amount instead.`;
    if (e.kind === "ZeroShares")
      return "Too small: this amount doesn't buy a single vault share.";
  }
  return "This deposit can't be sized against the vault.";
}

export type WithdrawSizing = {
  sharesIn: bigint;
  baseOut: bigint;
  quoteOut: bigint;
  minBaseOut: bigint;
  minQuoteOut: bigint;
};

// The shares a withdraw of `bps` / 10000 of a position redeems. A full
// withdraw takes every share, so the position closes and its rent comes back
// rather than leaving a rounding remainder behind.
export function sharesForFraction(shares: bigint, bps: number): bigint {
  // A non-finite fraction redeems nothing rather than throwing out of
  // BigInt, as applySlippage guards its own input.
  const finite = Number.isFinite(bps) ? Math.trunc(bps) : 0;
  const clamped = BigInt(
    Math.min(Math.max(finite, 0), Number(BPS_DENOMINATOR)),
  );
  return clamped === BPS_DENOMINATOR
    ? shares
    : (shares * clamped) / BPS_DENOMINATOR;
}

// The fee state a withdraw realizes before it slices.
export type VaultFeeState = Pick<
  VaultSummary,
  "leaderShares" | "hwm" | "perfFeeRatePpm" | "frozen"
>;

// The share count the program slices a withdraw against: the stored count
// plus the performance-fee shares `realize_in_place` mints first. A frozen
// vault realizes no fee, as the program's own guard has it.
export function realizedTotalShares(vault: VaultAtoms & VaultFeeState): bigint {
  if (vault.frozen) return vault.totalShares;
  return realizePerfFee(
    vault.baseAtoms,
    vault.quoteAtoms,
    vault.totalShares,
    vault.leaderShares,
    vault.hwm,
    BigInt(vault.perfFeeRatePpm),
  ).totalSharesAfter;
}

// Size a withdraw of `sharesIn` as the program's floored pro-rata slice of
// the realized vault, with each leg's floor the slice less the slippage
// tolerance.
export function sizeWithdraw(
  vault: VaultAtoms & VaultFeeState,
  sharesIn: bigint,
  slippageBps: number,
): WithdrawSizing {
  const [baseOut, quoteOut] = computeProRataSlice(
    sharesIn,
    realizedTotalShares(vault),
    vault.baseAtoms,
    vault.quoteAtoms,
  );
  return {
    sharesIn,
    baseOut,
    quoteOut,
    minBaseOut: applySlippage(baseOut, slippageBps),
    minQuoteOut: applySlippage(quoteOut, slippageBps),
  };
}

// `vault` as the program sees it once `realize_in_place` has minted the
// leader's performance fee, for every position figure (the table's and the
// dialog's), so each is sized against the share count a withdraw slices.
// Above the high-water mark the fee dilutes every position, so the stored
// count would overstate its value. The minted shares go to the leader, and
// `vps` scales down with the count, so the yield % nets the fee too; `chain`
// keeps the stored raw counts the SDK kernels realize from.
export function realizedVault(vault: Vault): Vault {
  const stored = vault.chain.totalShares;
  const realized = realizedTotalShares(vault.chain);
  if (realized === stored) return vault;
  return {
    ...vault,
    totalShares: Number(realized),
    leaderShares: Number(vault.chain.leaderShares + realized - stored),
    vps: (vault.vps * Number(stored)) / Number(realized),
  };
}

export type VaultRef = {
  // The market's base / quote symbols (the table's orientation).
  base: string;
  quote: string;
  // The vault's stable per-market number; its sector index may move.
  seq: bigint;
};

type VaultTxContext = {
  vault: VaultRef;
  owner: string;
  walletSession: WalletSession;
  rpc: Rpc;
  // Called once the transaction is built, as it goes to the wallet.
  onSigning?: () => void;
  slippageBps?: number;
};

// What the caller needs to read the position back once the transaction lands.
export type VaultTxOutcome = Submitted & {
  depositor: Address;
};

// Re-read the market and find the vault by `seq`, so the transaction is built
// against current state rather than the table's last poll.
async function freshVault(
  rpc: Rpc,
  vault: VaultRef,
): Promise<{ route: EclobRoute; summary: VaultSummary }> {
  const route = await resolveEclobRoute(
    rpc,
    stablecoinMint(vault.base),
    stablecoinMint(vault.quote),
  );
  if (!route) throw new SwapError("This vault's market is unavailable.", "api");
  await initSimulator();
  const summary = decodeDropsetVaults(route.marketData).find(
    (s) => s.seq === vault.seq,
  );
  if (!summary) throw new SwapError("This vault no longer exists.", "api");
  return { route, summary };
}

// The accounts both instructions share, plus idempotent creates for the
// signer's two ATAs (a withdraw may pay out a token the wallet has never held;
// a one-sided deposit still names both).
async function legAccounts(route: EclobRoute, signer: TransactionSigner) {
  const ata = async (owner: Address, mint: Address, tokenProgram: Address) =>
    (await findAssociatedTokenPda({ owner, mint, tokenProgram }))[0];
  const [
    signerBaseAta,
    signerQuoteAta,
    marketBaseTreasury,
    marketQuoteTreasury,
    createBaseAta,
    createQuoteAta,
  ] = await Promise.all([
    ata(signer.address, route.baseMint, route.baseTokenProgram),
    ata(signer.address, route.quoteMint, route.quoteTokenProgram),
    ata(route.market, route.baseMint, route.baseTokenProgram),
    ata(route.market, route.quoteMint, route.quoteTokenProgram),
    getCreateAssociatedTokenIdempotentInstructionAsync({
      payer: signer,
      owner: signer.address,
      mint: route.baseMint,
      tokenProgram: route.baseTokenProgram,
    }),
    getCreateAssociatedTokenIdempotentInstructionAsync({
      payer: signer,
      owner: signer.address,
      mint: route.quoteMint,
      tokenProgram: route.quoteTokenProgram,
    }),
  ]);
  return {
    accounts: {
      signer,
      market: route.market,
      baseMint: route.baseMint,
      quoteMint: route.quoteMint,
      baseTokenProgram: route.baseTokenProgram,
      quoteTokenProgram: route.quoteTokenProgram,
      signerBaseAta,
      signerQuoteAta,
      marketBaseTreasury,
      marketQuoteTreasury,
      program: route.programAddress,
    },
    createAtas: [createBaseAta, createQuoteAta],
  };
}

async function depositorPda(
  route: EclobRoute,
  vaultIdx: number,
  signer: Address,
): Promise<Address> {
  const [pda] = await findVaultDepositorPda(
    { market: route.market, vaultIdx, signer },
    { programAddress: route.programAddress },
  );
  return pda;
}

// Deposit `amountIn` atoms of `leg`; the program pulls the other leg pro rata.
// `walletAtoms` is the wallet's balance of each leg: the fresh basket is
// checked against it, and the bounds are capped at it, so a basket the
// wallet can't cover fails here with a reason instead of at the transfer.
export async function executeVaultDeposit({
  vault,
  owner,
  walletSession,
  rpc,
  onSigning,
  leg,
  amountIn,
  walletAtoms,
  slippageBps = VAULT_SLIPPAGE_BPS,
}: VaultTxContext & {
  leg: DepositLeg;
  amountIn: bigint;
  walletAtoms: { base: bigint; quote: bigint };
}): Promise<VaultTxOutcome> {
  assertWalletCanSign(walletSession);
  const { route, summary } = await freshVault(rpc, vault);
  const closed = depositClosedReason(summary, owner);
  if (closed) throw new SwapError(closed, "api");

  let sizing: DepositSizing;
  try {
    sizing = sizeDeposit(summary, leg, amountIn, slippageBps);
  } catch (e) {
    throw new SwapError(depositSizingError(e, leg, vault), "api");
  }
  if (!leaderFloorHolds(summary, sizing.sharesOut)) {
    throw new SwapError(
      "Too large: this would push the leader below their minimum stake.",
      "api",
    );
  }
  const short =
    sizing.baseIn > walletAtoms.base
      ? vault.base
      : sizing.quoteIn > walletAtoms.quote
        ? vault.quote
        : null;
  if (short) throw new SwapError(`Not enough ${short} in your wallet.`, "api");
  const cap = (bound: bigint, held: bigint) => (bound < held ? bound : held);

  const signer = createNoopSigner(address(owner));
  const { accounts, createAtas } = await legAccounts(route, signer);
  const depositIx = await getDepositInstructionAsync(
    {
      ...accounts,
      vaultIdx: summary.index,
      baseIn: leg === "base" ? amountIn : 0n,
      quoteIn: leg === "quote" ? amountIn : 0n,
      maxBaseIn: cap(sizing.maxBaseIn, walletAtoms.base),
      maxQuoteIn: cap(sizing.maxQuoteIn, walletAtoms.quote),
    },
    { programAddress: route.programAddress },
  );
  onSigning?.();
  const submitted = await submitInstructions({
    feePayer: signer,
    instructions: [...createAtas, depositIx],
    walletSession,
    rpc,
  });
  return {
    ...submitted,
    depositor: await depositorPda(route, summary.index, signer.address),
  };
}

// Withdraw `bps` / 10000 of the position, sized against the freshly read
// position and vault.
export async function executeVaultWithdraw({
  vault,
  owner,
  walletSession,
  rpc,
  onSigning,
  bps,
  slippageBps = VAULT_SLIPPAGE_BPS,
}: VaultTxContext & { bps: number }): Promise<VaultTxOutcome> {
  assertWalletCanSign(walletSession);
  const { route, summary } = await freshVault(rpc, vault);
  if (owner === summary.leader) {
    throw new SwapError(
      "You lead this vault. Leader withdrawals aren't made from this page.",
      "api",
    );
  }
  const signer = createNoopSigner(address(owner));
  const depositor = await depositorPda(route, summary.index, signer.address);
  const shares = await readDepositorShares(rpc, depositor);
  const sharesIn = sharesForFraction(shares, bps);
  if (shares === 0n) {
    throw new SwapError("You hold no shares in this vault to withdraw.", "api");
  }
  const sizing = sizeWithdraw(summary, sharesIn, slippageBps);
  // A fraction of a small position can round to no shares, or to shares
  // whose slice pays out nothing; either would burn shares for nothing.
  if (sharesIn === 0n || (sizing.baseOut === 0n && sizing.quoteOut === 0n)) {
    throw new SwapError(
      "Too small: this fraction of your position pays out nothing. Withdraw more.",
      "api",
    );
  }

  const { accounts, createAtas } = await legAccounts(route, signer);
  const withdrawIx = await getWithdrawInstructionAsync(
    {
      ...accounts,
      vaultDepositor: depositor,
      vaultIdx: summary.index,
      sharesIn,
      minBaseOut: sizing.minBaseOut,
      minQuoteOut: sizing.minQuoteOut,
    },
    { programAddress: route.programAddress },
  );
  onSigning?.();
  const submitted = await submitInstructions({
    feePayer: signer,
    instructions: [...createAtas, withdrawIx],
    walletSession,
    rpc,
  });
  return { ...submitted, depositor };
}

// The depositor's share balance as the chain holds it; 0 once a full
// withdraw has closed the account (or before a first deposit opens it).
export async function readDepositorShares(
  rpc: Rpc,
  depositor: Address,
): Promise<bigint> {
  const account = await fetchMaybeVaultDepositorHeader(rpc, depositor, {
    commitment: "finalized",
  });
  return account.exists ? account.data.shares : 0n;
}
