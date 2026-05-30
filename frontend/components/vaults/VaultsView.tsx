"use client";

import NumberFlow from "@number-flow/react";
import { useWalletConnection } from "@solana/react-hooks";
import { useMemo, useState } from "react";
import { ExternalLink } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import {
  SortableHeader,
  type SortDir,
  type SortState,
} from "@/components/ui/SortableHeader";
import { VaultActionDialog } from "@/components/vaults/VaultActionDialog";
import { shortenMint } from "@/lib/data/currencies";
import {
  ALL_VAULTS,
  type FxPairGroup,
  type GroupedVault,
  groupMetric,
  type MetricKey,
  VAULT_FX_GROUPS,
  type Vault,
  type VaultMarket,
  vaultApr24h,
  vaultMetric,
} from "@/lib/data/vaults";
import { useAppEvent } from "@/lib/events";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";
import { type Rgb, useFlagColor } from "@/lib/ui/flagColor";

const COLSPAN = 8;

const APR_TOOLTIP =
  "Annualized returns for depositors, based on the fees this vault accrued over the last 24 hours.";

// Pin the generic shared header to this table's metric keys so the literal
// `sortKey` props type-check against `sort` / `onToggle`.
const VaultSortHeader = SortableHeader<MetricKey>;

// Order two metric values; nulls (zero-TVL APR) always sink to the bottom
// regardless of direction.
const cmpMetric = (
  a: number | null,
  b: number | null,
  direction: SortDir,
): number => {
  if (a === null && b === null) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  return direction === "desc" ? b - a : a - b;
};

// Average two flag colors for the group underline. Falls back to whichever
// single color resolved (the other may still be rastering or have no
// saturated band).
const averageRgb = (a: Rgb | null, b: Rgb | null): Rgb | null => {
  if (a && b)
    return [(a[0] + b[0]) >> 1, (a[1] + b[1]) >> 1, (a[2] + b[2]) >> 1];
  return a ?? b;
};

// Compact USD ("$1.2M") cell. `null` → em dash.
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

// APR cell — em dash when null (zero TVL). Toned positive so the yield reads
// as an at-a-glance "good" signal.
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

// The connected user's deposit in a vault. No indexer yet, so it reads an em
// dash when disconnected and $0.00 when connected (no positions in the mock).
function DepositCell({ connected }: { connected: boolean }) {
  return (
    <td className="border-border border-r px-3 py-2 text-right align-middle font-mono text-foreground tabular-nums last:border-r-0">
      {connected ? (
        <NumberFlow value={0} format={FORMATS.usd} />
      ) : (
        <span className="text-muted-fg">—</span>
      )}
    </td>
  );
}

// Two flags rendered as full SVGs side by side. The Twemoji artwork is already
// a rounded rectangle, so we don't clip it to a circle (that produced a stray
// border on square flags like CH).
function FlagPair({
  base,
  quote,
  size,
}: {
  base: string;
  quote: string;
  size: number;
}) {
  return (
    <span className="flex shrink-0 items-center gap-1">
      {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
      <img src={base} alt="" aria-hidden width={size} height={size} />
      {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
      <img src={quote} alt="" aria-hidden width={size} height={size} />
    </span>
  );
}

// Two overlapping circular token icons (the stablecoin logos are round).
function TokenPair({
  base,
  quote,
  size,
}: {
  base: string;
  quote: string;
  size: number;
}) {
  return (
    <span className="flex shrink-0 items-center">
      {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
      <img
        src={base}
        alt=""
        aria-hidden
        width={size}
        height={size}
        className="rounded-full"
      />
      {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
      <img
        src={quote}
        alt=""
        aria-hidden
        width={size}
        height={size}
        className="-ml-1.5 rounded-full ring-1 ring-background"
      />
    </span>
  );
}

// FX-pair heading spanning the table, mirroring the per-currency headings on
// /currencies: two flags + "EUR / USD" + nickname, with the pair's summed TVL
// / 24h volume / vault count to the right. The bottom edge is tinted with the
// averaged dominant color of the two flags (an inset box-shadow rather than a
// border, which would collapse with the cell separators below).
function FxGroupHeading({ group }: { group: FxPairGroup }) {
  const baseColor = useFlagColor(group.baseCurrency, group.baseFlagUrl);
  const quoteColor = useFlagColor(group.quoteCurrency, group.quoteFlagUrl);
  const avg = averageRgb(baseColor, quoteColor);
  const style = avg
    ? { boxShadow: `inset 0 -2px 0 rgb(${avg[0]} ${avg[1]} ${avg[2]} / 0.6)` }
    : undefined;
  const count = group.vaults.length;
  return (
    <tr className="bg-background">
      <td colSpan={COLSPAN} className="px-3 pt-8 pb-3" style={style}>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <FlagPair
            base={group.baseFlagUrl}
            quote={group.quoteFlagUrl}
            size={32}
          />
          <span className="font-semibold text-foreground text-xl">
            {group.label}
          </span>
          {group.nickname && (
            <span className="text-muted-fg text-base">“{group.nickname}”</span>
          )}
          <span className="ml-auto flex items-center gap-3 font-mono text-muted-fg text-xs tabular-nums">
            <span>
              TVL <NumberFlow value={group.tvl} format={FORMATS.usdCompact} />
            </span>
            <span>
              Vol{" "}
              <NumberFlow value={group.volume24h} format={FORMATS.usdCompact} />
            </span>
            <span>
              {count} {count === 1 ? "vault" : "vaults"}
            </span>
          </span>
        </div>
      </td>
    </tr>
  );
}

// One vault row. The pair/token column leads with the market glyphs; the
// leader has its own column. A single "Manage" button opens the deposit /
// withdraw modal — always available (a frozen vault is withdraw-only, which
// the modal enforces).
function VaultRow({
  entry,
  grouped,
  connected,
  onManage,
}: {
  entry: GroupedVault;
  grouped: boolean;
  connected: boolean;
  onManage: (market: VaultMarket, vault: Vault) => void;
}) {
  const { market, vault } = entry;
  return (
    <tr className="border-border border-t bg-muted/40">
      <td
        className={`border-border border-r py-2 pr-3 align-middle last:border-r-0 ${grouped ? "pl-10" : "pl-3"}`}
      >
        <div className="flex items-center gap-2">
          {!grouped && (
            <FlagPair
              base={market.baseFlagUrl}
              quote={market.quoteFlagUrl}
              size={16}
            />
          )}
          <TokenPair
            base={market.baseIconUrl}
            quote={market.quoteIconUrl}
            size={18}
          />
          <span className="font-mono font-medium text-foreground">
            {market.label}
          </span>
        </div>
      </td>
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 align-middle last:border-r-0">
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
            <span className="rounded bg-accent-sell/15 px-1.5 py-0.5 font-medium text-[10px] text-accent-sell uppercase tracking-wide">
              Frozen
            </span>
          )}
        </div>
      </td>
      <UsdCell value={vault.tvl} />
      <UsdCell value={vault.volume24h} />
      <UsdCell value={vault.fees24h} />
      <AprCell apr={vaultApr24h(vault)} />
      <DepositCell connected={connected} />
      <td className="px-3 py-2 text-right align-middle">
        <button
          type="button"
          onClick={() => onManage(market, vault)}
          title={
            vault.frozen ? "Frozen — withdrawals only" : "Deposit or withdraw"
          }
          className="rounded border border-border bg-background px-3 py-1 font-medium text-foreground text-xs transition-colors hover:border-accent hover:text-accent"
        >
          Manage
        </button>
      </td>
    </tr>
  );
}

export function VaultsView() {
  const { connected } = useWalletConnection();
  const [groupByPair, setGroupByPair] = useState(true);
  const [sort, setSort] = useState<SortState<MetricKey>>(null);
  const [dialog, setDialog] = useState<{
    market: VaultMarket;
    vault: Vault;
  } | null>(null);

  // There's always an effective sort; default 24h volume desc.
  const effective: { key: MetricKey; direction: SortDir } = sort ?? {
    key: "volume24h",
    direction: "desc",
  };

  const toggleSort = (key: MetricKey) =>
    setSort((prev) => {
      if (!prev || prev.key !== key) return { key, direction: "desc" };
      if (prev.direction === "desc") return { key, direction: "asc" };
      return null;
    });

  // Keyboard shortcuts (see lib/ui/shortcuts.ts → vaults context).
  useAppEvent("toggleGroupByPair", () => setGroupByPair((g) => !g));
  useAppEvent("vaultsSort", (key) => toggleSort(key));

  // Grouped: sort the groups by aggregate, and each group's vaults by the same
  // metric.
  const groups = useMemo(() => {
    const { key, direction } = effective;
    return [...VAULT_FX_GROUPS]
      .sort((a, b) =>
        cmpMetric(groupMetric(a, key), groupMetric(b, key), direction),
      )
      .map((g) => ({
        group: g,
        vaults: [...g.vaults].sort((a, b) =>
          cmpMetric(vaultMetric(a, key), vaultMetric(b, key), direction),
        ),
      }));
  }, [effective]);

  // Ungrouped: one flat, sorted list of every vault.
  const flatVaults = useMemo(() => {
    const { key, direction } = effective;
    return [...ALL_VAULTS].sort((a, b) =>
      cmpMetric(vaultMetric(a, key), vaultMetric(b, key), direction),
    );
  }, [effective]);

  const onManage = (market: VaultMarket, vault: Vault) =>
    setDialog({ market, vault });

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      <div className="mb-3 flex items-end justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <h1 className="font-semibold text-foreground text-lg">Vaults</h1>
            <span className="rounded-full border border-accent/40 bg-accent/10 px-2 py-0.5 font-medium text-[10px] text-accent uppercase tracking-wide">
              Preview
            </span>
          </div>
          <p className="text-muted-fg text-sm">
            Back a leader's vault and share in spread capture.{" "}
            <span className="text-muted-fg/80">
              All figures shown are mock data.
            </span>
          </p>
        </div>
        <label className="flex select-none items-center gap-2 text-muted-fg text-xs hover:text-foreground">
          <input
            type="checkbox"
            checked={groupByPair}
            onChange={(e) => setGroupByPair(e.target.checked)}
            className="h-3.5 w-3.5 cursor-pointer accent-accent"
          />
          Group by pair
        </label>
      </div>
      <div className="rounded-lg border border-border">
        <table className="w-full min-w-[940px] text-left text-sm">
          <thead className="text-muted-fg text-xs uppercase">
            <tr>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0"
              >
                Pair
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 w-px whitespace-nowrap border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0"
              >
                Leader
              </th>
              <VaultSortHeader
                sortKey="tvl"
                label="TVL"
                sort={sort}
                onToggle={toggleSort}
              />
              <VaultSortHeader
                sortKey="volume24h"
                label="24h Vol"
                sort={sort}
                onToggle={toggleSort}
              />
              <VaultSortHeader
                sortKey="fees24h"
                label="24h Fees"
                sort={sort}
                onToggle={toggleSort}
              />
              <VaultSortHeader
                sortKey="apr24h"
                label="APR 24h"
                sort={sort}
                onToggle={toggleSort}
                info={APR_TOOLTIP}
              />
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 text-right font-medium last:border-r-0"
              >
                Your Deposit
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 bg-muted px-3 py-2 text-right font-medium"
              >
                Position
              </th>
            </tr>
          </thead>
          <tbody>
            {groupByPair
              ? groups.flatMap(({ group, vaults }) => [
                  <FxGroupHeading key={`h-${group.key}`} group={group} />,
                  ...vaults.map((entry) => (
                    <VaultRow
                      key={entry.vault.vaultPubkey}
                      entry={entry}
                      grouped
                      connected={connected}
                      onManage={onManage}
                    />
                  )),
                ])
              : flatVaults.map((entry) => (
                  <VaultRow
                    key={entry.vault.vaultPubkey}
                    entry={entry}
                    grouped={false}
                    connected={connected}
                    onManage={onManage}
                  />
                ))}
          </tbody>
        </table>
      </div>
      {dialog && (
        <VaultActionDialog
          market={dialog.market}
          vault={dialog.vault}
          connected={connected}
          open={true}
          onOpenChange={(open) => {
            if (!open) setDialog(null);
          }}
        />
      )}
    </div>
  );
}
