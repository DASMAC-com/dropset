"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { CURRENCIES, type IsoCurrencyCode } from "@/lib/currencies";
import { BUY_TINT, SELL_TINT } from "@/lib/globeConstants";
import { useSwapStore } from "@/lib/store";
import { CurrencyGroupHeader } from "./CurrencyGroupHeader";
import { X } from "./icons";
import { PickerBalanceCell } from "./PickerBalanceCell";
import { StableTokenIdentity } from "./StableTokenIdentity";

export type ClickContext = {
  countryName: string;
  cca2: string;
  currencies: IsoCurrencyCode[];
};

// Globe-anchored token picker. Opens when the user clicks a country
// polygon / pin / flag. Lists every stablecoin that pegs to one of the
// country's currencies, with From/To buttons that go straight to the
// resolved swap URL.
//
// Position: when `top` is provided, the dialog renders flush with that
// viewport coordinate (so it lines up with the top of the globe), with
// height clamped to fit the remaining viewport. Falls back to centered
// when the caller doesn't yet have a measurement.
export function CountryPickerDialog({
  ctx,
  top,
  onClose,
  onPick,
}: {
  ctx: ClickContext | null;
  top: number | null;
  onClose: () => void;
  onPick: (
    side: "from" | "to",
    currency: IsoCurrencyCode,
    symbol: string,
    cca2: string,
  ) => void;
}) {
  const from = useSwapStore((s) => s.from);
  const to = useSwapStore((s) => s.to);

  return (
    <Dialog.Root
      open={ctx !== null}
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-[60] bg-background/80 backdrop-blur-2xl" />
        <Dialog.Content
          aria-describedby={undefined}
          style={
            top !== null
              ? { top, maxHeight: `calc(100vh - ${top}px - 1rem)` }
              : undefined
          }
          className={`-translate-x-1/2 fixed left-1/2 z-[70] flex w-fit max-w-[calc(100vw-2rem)] flex-col overflow-hidden rounded-xl border border-border bg-background shadow-lg ${
            top === null
              ? "-translate-y-1/2 top-1/2 max-h-[calc(100vh-3rem)]"
              : ""
          }`}
        >
          <div className="flex items-center gap-2 border-border border-b px-3 py-2">
            <Dialog.Title className="min-w-0 flex-1 truncate text-foreground text-sm">
              {ctx?.countryName ?? ""}
            </Dialog.Title>
            <kbd className="hidden shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-fg sm:inline-block">
              Esc
            </kbd>
            <Dialog.Close
              aria-label="Close"
              className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-muted hover:text-foreground"
            >
              <X size={14} />
            </Dialog.Close>
          </div>
          <div className="flex-1 overflow-y-auto p-1">
            {ctx?.currencies.map((cur) => (
              <div key={cur} className="py-1">
                <CurrencyGroupHeader code={cur} />
                {CURRENCIES[cur].stablecoins.map((s) => {
                  const isFromHere =
                    cur === from.currency && s.symbol === from.stablecoin;
                  const isToHere =
                    cur === to.currency && s.symbol === to.stablecoin;
                  return (
                    <div
                      key={`${cur}-${s.symbol}`}
                      className="flex w-full items-center gap-1 rounded-md px-2 py-1.5"
                    >
                      <StableTokenIdentity
                        s={s}
                        symbolClassName="text-foreground"
                      />
                      <PickerBalanceCell
                        mint={s.mint}
                        decimals={s.decimals}
                        symbol={s.symbol}
                      />
                      <button
                        type="button"
                        disabled={isToHere}
                        onClick={() => onPick("from", cur, s.symbol, ctx.cca2)}
                        title={
                          isToHere
                            ? "Already selected as To"
                            : `Swap from ${s.symbol} (${ctx.countryName})`
                        }
                        style={
                          isFromHere
                            ? { backgroundColor: SELL_TINT }
                            : undefined
                        }
                        className={`shrink-0 rounded px-2 py-1 text-center font-medium text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                          isFromHere
                            ? "text-white"
                            : "border border-border text-muted-fg hover:border-[#3b82f6] hover:text-[#3b82f6]"
                        }`}
                      >
                        From
                      </button>
                      <button
                        type="button"
                        disabled={isFromHere}
                        onClick={() => onPick("to", cur, s.symbol, ctx.cca2)}
                        title={
                          isFromHere
                            ? "Already selected as From"
                            : `Swap to ${s.symbol} (${ctx.countryName})`
                        }
                        style={
                          isToHere ? { backgroundColor: BUY_TINT } : undefined
                        }
                        className={`shrink-0 rounded px-2 py-1 text-center font-medium text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                          isToHere
                            ? "text-white"
                            : "border border-border text-muted-fg hover:border-[#10b981] hover:text-[#10b981]"
                        }`}
                      >
                        To
                      </button>
                    </div>
                  );
                })}
              </div>
            ))}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
