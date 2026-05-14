"use client";

import { useSameToken } from "@/lib/store";
import { SwapArrowButton } from "./SwapArrowButton";
import { TokenRow } from "./TokenRow";

export function SwapPanel() {
  const sameToken = useSameToken();

  return (
    <div className="relative rounded-xl border border-border p-4">
      <div className="relative flex flex-col gap-2">
        <TokenRow side="from" label="From" />
        <TokenRow side="to" label="To" />
        <div className="absolute inset-x-0 top-1/2 z-10 flex -translate-y-1/2 items-center justify-center">
          <SwapArrowButton />
        </div>
      </div>
      <button
        type="button"
        onClick={() => {}}
        disabled={sameToken}
        title={sameToken ? "Pick a different token on one side" : undefined}
        className="mt-4 w-full rounded-lg bg-accent px-4 py-2.5 font-medium text-background text-sm transition-colors hover:bg-accent-hover disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-fg disabled:hover:bg-muted"
      >
        {sameToken ? "Pick a different token" : "Swap"}
      </button>
    </div>
  );
}
