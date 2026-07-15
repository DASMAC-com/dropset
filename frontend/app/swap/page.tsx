"use client";

import dynamic from "next/dynamic";
import { Suspense } from "react";
import { GlobePanel } from "@/components/globe/GlobePanel";
import { UrlSync } from "@/components/swap/UrlSync";

// SwapPanel is loaded client-only via next/dynamic so its store-derived
// content (token symbols, currency names, flag images) is never SSR-rendered
// — eliminating the hydration mismatch on URL-deep-linked loads. The
// SwapStoreProvider's useRef factory reads ?from/?to on first client mount,
// so SwapPanel's first render lands on the URL-derived pair with no flash.
// Trade-off: a brief blank where the panel slot is, until the chunk loads
// — same pattern GlobePanel already uses for its three.js dependency.
const SwapPanel = dynamic(
  () =>
    import("@/components/swap/SwapPanel").then((m) => ({
      default: m.SwapPanel,
    })),
  { ssr: false },
);

// Client-only for the same reason as SwapPanel: it reads the pair from the
// swap store (seeded from the URL on first client mount) and polls the chain,
// neither of which SSR can do. It self-hides until a live market exists for
// the pair, so it costs nothing in the layout otherwise.
const OrderBookPanel = dynamic(
  () =>
    import("@/components/orderbook/OrderBook").then((m) => ({
      default: m.OrderBookPanel,
    })),
  { ssr: false },
);

// UrlSync uses useSearchParams as a re-render signal for same-path
// different-query navigation, which Next.js requires be wrapped in a
// Suspense boundary so the static prerender can stream around it.
export default function SwapPage() {
  return (
    <div className="mx-auto flex w-full max-w-[960px] flex-col items-center gap-4 px-6 pt-3 pb-10 lg:flex-row lg:items-start lg:justify-center">
      <div className="flex w-full max-w-[575px] flex-col gap-3">
        <Suspense fallback={null}>
          <UrlSync />
        </Suspense>
        <SwapPanel />
        <GlobePanel />
      </div>
      {/* Order book beside the swap it feeds: to the right on wide screens,
          stacked below on narrow ones. Only renders when a live market exists
          for the pair (the panel self-hides otherwise). */}
      <OrderBookPanel className="w-full max-w-[575px] lg:w-[320px] lg:shrink-0" />
    </div>
  );
}
