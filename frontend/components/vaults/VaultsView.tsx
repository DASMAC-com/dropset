"use client";

import NumberFlow from "@number-flow/react";
import { useState } from "react";
import { ChevronDown, ExternalLink, Minus, Plus } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import {
  VaultActionDialog,
  type VaultActionMode,
} from "@/components/vaults/VaultActionDialog";
import { shortenMint, tokenIconUrl } from "@/lib/data/currencies";
import {
  type FxPairGroup,
  marketVolume24h,
  VAULT_FX_GROUPS,
  type Vault,
  type VaultMarket,
  vaultApr24h,
} from "@/lib/data/vaults";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";

const COLSPAN = 5;

// Compact USD ("$1.2M") number cell. `null` → em dash, used where a stat
// doesn't apply at a given tier (e.g. per-vault volume, which we report at the
// market/pair level instead).
function UsdCell({ value }: { value: number | null }) {
  return (
    <td className="border-border border-r px-3 py-2 text-right align-middle font-mono text-foreground tabular-nums last:border-r-0">
      {value === null ? (
        <span className="text-muted-fg">—</span>
      ) : (
        <NumberFlow value={value} format={FORMATS.usdCompact} />
      )}
    </td>
  );
}

// APR cell — em dash when null (zero TVL or N/A at this tier). Toned positive
// so the yield reads as an at-a-glance "good" signal.
function AprCell({ apr }: { apr: number | null }) {
  return (
    <td className="border-border border-r px-3 py-2 text-right align-middle font-mono tabular-nums text-accent-buy last:border-r-0">
      {apr === null ? (
        <span className="text-muted-fg">—</span>
      ) : (
        <NumberFlow value={apr} format={FORMATS.percent} />
      )}
    </td>
  );
}

// FX-pair group header — two flags + "EUR / USD" + fiat names, spanning the
// table. Mirrors the per-currency header rows on /currencies.
function FxGroupHeaderRow({ group }: { group: FxPairGroup }) {
  return (
    <tr className="bg-background">
      <td colSpan={COLSPAN} className="px-3 pt-8 pb-3">
        <div className="flex items-center gap-3">
          <span aria-hidden className="flex shrink-0 items-center">
            {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
            <img
              src={group.baseFlagUrl}
              alt=""
              aria-hidden
              width={32}
              height={32}
              className="rounded-md"
            />
            {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
            <img
              src={group.quoteFlagUrl}
              alt=""
              aria-hidden
              width={32}
              height={32}
              className="-ml-2 rounded-md ring-2 ring-background"
            />
          </span>
          <span className="font-semibold text-foreground text-xl">
            {group.label}
          </span>
          <span className="text-muted-fg">·</span>
          <span className="text-muted-fg text-base">
            {group.baseName} / {group.quoteName}
          </span>
        </div>
      </td>
    </tr>
  );
}

// Stablecoin-market row (e.g. EURC/USDC). Shows 24h volume summed across the
// market's vaults; fees/APR/leader live on the per-vault rows below, so this
// row's fees/APR cells stay blank and it expands to reveal them.
function MarketRow({
  market,
  expanded,
  onToggleExpand,
}: {
  market: VaultMarket;
  expanded: boolean;
  onToggleExpand: () => void;
}) {
  const count = market.vaults.length;
  return (
    <tr
      className="cursor-pointer border-border border-t hover:bg-muted/40"
      onClick={onToggleExpand}
    >
      <td className="border-border border-r py-2 pr-3 pl-6 align-middle last:border-r-0">
        <div className="flex items-center gap-2">
          <ChevronDown
            size={14}
            className={`shrink-0 text-muted-fg transition-transform ${expanded ? "" : "-rotate-90"}`}
          />
          <span className="flex shrink-0 items-center">
            {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
            <img
              src={tokenIconUrl(market.base)}
              alt=""
              aria-hidden
              width={20}
              height={20}
              className="h-5 w-5 rounded-full"
            />
            {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
            <img
              src={tokenIconUrl(market.quote)}
              alt=""
              aria-hidden
              width={20}
              height={20}
              className="-ml-1.5 h-5 w-5 rounded-full ring-1 ring-background"
            />
          </span>
          <span className="font-mono font-medium text-foreground">
            {market.label}
          </span>
        </div>
      </td>
      <UsdCell value={marketVolume24h(market)} />
      <UsdCell value={null} />
      <AprCell apr={null} />
      <td className="px-3 py-2 text-right align-middle text-muted-fg text-xs">
        {count} {count === 1 ? "vault" : "vaults"}
      </td>
    </tr>
  );
}

// Per-vault row (one leader). Volume is reported at the market tier, so it's
// blank here; fees + APR are the per-vault stats, plus deposit/withdraw.
function VaultRow({
  vault,
  onAction,
}: {
  vault: Vault;
  onAction: (vault: Vault, mode: VaultActionMode) => void;
}) {
  const depositDisabled = vault.frozen || !vault.outsideDepositsApproved;
  const withdrawDisabled = vault.frozen;
  const actionBtn = (mode: VaultActionMode, disabled: boolean) => {
    const Icon = mode === "deposit" ? Plus : Minus;
    const title = vault.frozen
      ? "This vault is frozen"
      : mode === "deposit" && !vault.outsideDepositsApproved
        ? "Outside deposits not approved for this vault"
        : mode === "deposit"
          ? "Deposit into this vault"
          : "Withdraw from this vault";
    return (
      <button
        type="button"
        onClick={() => onAction(vault, mode)}
        disabled={disabled}
        title={title}
        className="inline-flex items-center gap-1 rounded border border-border bg-background px-2 py-1 font-medium text-foreground text-xs transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:border-border disabled:bg-muted disabled:text-muted-fg disabled:hover:border-border disabled:hover:text-muted-fg"
      >
        <Icon size={12} />
        {mode === "deposit" ? "Deposit" : "Withdraw"}
      </button>
    );
  };
  return (
    <tr className="border-border border-t bg-muted/40">
      <td className="border-border border-r py-2 pr-3 pl-14 align-middle last:border-r-0">
        <div className="flex items-center gap-1">
          <span
            className="font-mono text-foreground text-xs"
            title={vault.leader}
          >
            {shortenMint(vault.leader)}
          </span>
          <CopyButton value={vault.leader} label="leader address" />
          <a
            href={explorerAddressUrl(vault.leader)}
            target="_blank"
            rel="noopener noreferrer"
            title="View leader on Solscan"
            className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
          >
            <ExternalLink size={12} />
          </a>
          {vault.frozen && (
            <span className="ml-1 rounded bg-accent-sell/15 px-1.5 py-0.5 font-medium text-[10px] text-accent-sell uppercase tracking-wide">
              Frozen
            </span>
          )}
        </div>
      </td>
      <UsdCell value={null} />
      <UsdCell value={vault.fees24h} />
      <AprCell apr={vaultApr24h(vault)} />
      <td className="px-3 py-2 text-right align-middle">
        <div className="flex items-center justify-end gap-1">
          {actionBtn("deposit", depositDisabled)}
          {actionBtn("withdraw", withdrawDisabled)}
        </div>
      </td>
    </tr>
  );
}

export function VaultsView() {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [dialog, setDialog] = useState<{
    market: VaultMarket;
    vault: Vault;
    mode: VaultActionMode;
  } | null>(null);

  const toggleExpand = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      <div className="mb-3">
        <h1 className="font-semibold text-foreground text-lg">Vaults</h1>
        <p className="text-muted-fg text-sm">
          Back a leader's vault and share in spread capture. Pairs group by FX
          rate; expand a market to see its vaults.
        </p>
      </div>
      <div className="rounded-lg border border-border">
        <table className="w-full min-w-[720px] text-left text-sm">
          <thead className="text-muted-fg text-xs uppercase">
            <tr>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0"
              >
                Pair / Leader
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 text-right font-medium last:border-r-0"
              >
                24h Vol
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 text-right font-medium last:border-r-0"
              >
                24h Fees
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 text-right font-medium last:border-r-0"
              >
                APR 24h
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 bg-muted px-3 py-2 text-right font-medium"
              >
                Vaults
              </th>
            </tr>
          </thead>
          <tbody>
            {VAULT_FX_GROUPS.flatMap((group) => [
              <FxGroupHeaderRow key={`h-${group.key}`} group={group} />,
              ...group.markets.flatMap((market) => {
                const isOpen = expanded.has(market.marketPubkey);
                return [
                  <MarketRow
                    key={market.marketPubkey}
                    market={market}
                    expanded={isOpen}
                    onToggleExpand={() => toggleExpand(market.marketPubkey)}
                  />,
                  ...(isOpen
                    ? market.vaults.map((vault) => (
                        <VaultRow
                          key={vault.vaultPubkey}
                          vault={vault}
                          onAction={(v, mode) =>
                            setDialog({ market, vault: v, mode })
                          }
                        />
                      ))
                    : []),
                ];
              }),
            ])}
          </tbody>
        </table>
      </div>
      {dialog && (
        <VaultActionDialog
          market={dialog.market}
          vault={dialog.vault}
          mode={dialog.mode}
          open={true}
          onOpenChange={(open) => {
            if (!open) setDialog(null);
          }}
        />
      )}
    </div>
  );
}
