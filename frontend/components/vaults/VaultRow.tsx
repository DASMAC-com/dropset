import NumberFlow from "@number-flow/react";
import { Crosshair, RefreshCw } from "@/components/icons";
import { FlagPair } from "@/components/ui/Flag";
import { TokenIcon } from "@/components/ui/TokenIcon";
import { allTimePnl, positionPnl } from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import {
  contributeClosedReason,
  type GroupedVault,
  type Vault,
  type VaultMarket,
  vaultLabel,
  vaultNumberLabel,
  vaultRefPrice,
} from "@/lib/data/vaults";
import { realizedVault } from "@/lib/eclob/vaultTx";
import { emit } from "@/lib/events";
import { FORMATS } from "@/lib/format/formats";
import { groupedRowClassName } from "@/lib/ui/groupedRows";
import { pnlTone } from "@/lib/ui/pnlTone";
import { useGoToSwapPair } from "@/lib/ui/swapUrl";

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

// The connected user's position value in a vault, marked at the vault's
// reference price (vaultRefPrice), with the all-time return %
// below it (same red/green as the dialog's headline). The basket breakdown
// lives in the manage dialog.
function PositionValue({
  vault,
  position,
}: {
  vault: Vault;
  position: VaultPosition;
}) {
  const refNow = vaultRefPrice(vault) ?? position.entryRefPrice;
  // Against the fee-realized share count, so it matches what a withdraw pays.
  const realized = realizedVault(vault);
  const { currentValue } = positionPnl(position, realized, refNow);
  const at = allTimePnl(position, realized, refNow);
  return (
    <span className="flex flex-col items-end font-mono text-xs tabular-nums">
      <span className="text-foreground">
        <NumberFlow value={currentValue} format={FORMATS.usd} />
      </span>
      <span
        className={`text-[10px] ${pnlTone(at.allTimePnl, "text-muted-fg")}`}
      >
        (<NumberFlow value={at.allTimePct} format={FORMATS.signedReturn} />)
      </span>
    </span>
  );
}

// Two overlapping circular token icons (the stablecoin logos are round).
// Named `*Symbol` because the sibling FlagPair on the same row takes URLs, and
// a bare `base`/`quote` pair would make the two indistinguishable at the call
// site; TokenIcon resolves each symbol's mirror and remote fallback itself.
function TokenPair({
  baseSymbol,
  quoteSymbol,
  size,
}: {
  baseSymbol: string;
  quoteSymbol: string;
  size: number;
}) {
  return (
    <span className="flex shrink-0 items-center">
      <TokenIcon symbol={baseSymbol} size={size} className="rounded-full" />
      <TokenIcon
        symbol={quoteSymbol}
        size={size}
        className="-ml-1.5 rounded-full ring-1 ring-background"
      />
    </span>
  );
}

// One vault row. The action button is context-aware: connect a wallet first,
// then deposit if you hold no position, or withdraw if you do.
export function VaultRow({
  entry,
  grouped,
  connected,
  position,
  pinned,
  rowIndex,
  groupSize,
  onManage,
  onPin,
}: {
  entry: GroupedVault;
  grouped: boolean;
  connected: boolean;
  position: VaultPosition | null;
  // Whether the URL filter is pinned to exactly this vault.
  pinned: boolean;
  rowIndex: number;
  groupSize: number;
  onManage: (market: VaultMarket, vault: Vault) => void;
  onPin: (entry: GroupedVault) => void;
}) {
  const { market, vault } = entry;
  const goToSwapPair = useGoToSwapPair();
  const closedReason = contributeClosedReason(market, vault);

  const action = !connected
    ? {
        label: "Connect",
        disabled: false,
        onClick: () => emit("openWalletModal"),
      }
    : position
      ? {
          // A held position can be topped off or withdrawn, so the dialog is a
          // general "Manage", not just "Withdraw".
          label: "Manage",
          disabled: false,
          onClick: () => onManage(market, vault),
        }
      : {
          label: "Deposit",
          disabled: closedReason !== null,
          onClick: () => onManage(market, vault),
        };
  const actionTitle =
    action.label === "Deposit" && action.disabled
      ? (closedReason ?? undefined)
      : undefined;

  return (
    <tr className={groupedRowClassName(rowIndex, groupSize)}>
      <td className="whitespace-nowrap border-border border-r px-3 py-2 align-middle last:border-r-0">
        <div className="flex items-center gap-2">
          {!grouped && (
            <FlagPair
              base={market.baseFlagUrl}
              quote={market.quoteFlagUrl}
              size={16}
            />
          )}
          <TokenPair
            baseSymbol={market.base}
            quoteSymbol={market.quote}
            size={28}
          />
          <span className="font-mono font-medium text-foreground">
            {market.label}
          </span>
          {/* Jump to /swap with this pair loaded into the store. */}
          <button
            type="button"
            onClick={() =>
              goToSwapPair(
                { currency: market.baseCurrency, stablecoin: market.base },
                { currency: market.quoteCurrency, stablecoin: market.quote },
              )
            }
            title={`Swap ${market.label}`}
            aria-label={`Swap ${market.label}`}
            className="ml-1 inline-flex shrink-0 items-center rounded p-1 text-muted-fg transition-colors hover:bg-muted hover:text-accent"
          >
            <RefreshCw size={14} />
          </button>
        </div>
      </td>
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 align-middle last:border-r-0">
        <div className="flex items-center gap-1">
          {/* For display the vault number is the identity, so it leads,
              ahead of the leader-set name, which can't then pass for another
              vault's number (the pin keys on the leader). The hover
              keeps the full address; its copy and explorer links live in
              the dialog. Clicking the number or name toggles the pin like
              the crosshair; the crosshair stays the one keyboard stop. */}
          <button
            type="button"
            tabIndex={-1}
            onClick={() => onPin(entry)}
            className="flex min-w-0 cursor-pointer items-center gap-1"
          >
            <span
              className="shrink-0 font-mono text-foreground text-xs"
              title={`${vaultLabel(vault)} — Leader ${vault.leader}`}
            >
              {vaultNumberLabel(vault)}
            </span>
            {vault.name && (
              <span className="max-w-48 truncate font-medium text-foreground text-sm">
                {vault.name}
              </span>
            )}
          </button>
          {/* Pin the view to this exact vault — writes base/quote and the
              leader's address into the URL (a shareable deep link); click
              again to clear. */}
          <button
            type="button"
            onClick={() => onPin(entry)}
            aria-label={pinned ? "Clear vault filter" : "Pin this vault"}
            title={
              pinned
                ? "Clear vault filter"
                : "Pin this vault — filters the table and updates the shareable URL"
            }
            className={`inline-flex shrink-0 items-center rounded p-1 transition-colors hover:bg-muted ${
              pinned ? "text-accent" : "text-muted-fg hover:text-accent"
            }`}
          >
            <Crosshair size={12} />
          </button>
          {vault.frozen && (
            <span className="rounded bg-accent-sell/15 px-1.5 py-0.5 font-medium text-[10px] text-accent-sell uppercase tracking-wide">
              Frozen
            </span>
          )}
        </div>
      </td>
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 text-right align-middle last:border-r-0">
        {connected ? (
          position ? (
            <PositionValue vault={vault} position={position} />
          ) : (
            <span className="font-mono text-muted-fg text-xs">$-</span>
          )
        ) : (
          <span className="font-mono text-muted-fg text-xs">—</span>
        )}
      </td>
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 text-right align-middle last:border-r-0">
        <button
          type="button"
          onClick={action.onClick}
          disabled={action.disabled}
          title={actionTitle}
          className="shrink-0 rounded border border-border bg-background px-3 py-1 font-medium text-foreground text-xs transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:border-border disabled:bg-muted disabled:text-muted-fg disabled:hover:border-border disabled:hover:text-muted-fg"
        >
          {action.label}
        </button>
      </td>
      <UsdCell value={vault.tvl} />
    </tr>
  );
}
