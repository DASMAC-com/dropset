"use client";

import dynamic from "next/dynamic";
import { Suspense } from "react";
import { GlobePanel } from "@/components/GlobePanel";
import { UrlSync } from "@/components/UrlSync";

// SwapPanel is loaded client-only via next/dynamic so its store-derived
// content (token symbols, currency names, flag images) is never SSR-rendered
// — eliminating the hydration mismatch on URL-deep-linked loads. The
// SwapStoreProvider's useRef factory reads ?from/?to on first client mount,
// so SwapPanel's first render lands on the URL-derived pair with no flash.
// Trade-off: a brief blank where the panel slot is, until the chunk loads
// — same pattern GlobePanel already uses for its three.js dependency.
const SwapPanel = dynamic(
  () =>
    import("@/components/SwapPanel").then((m) => ({ default: m.SwapPanel })),
  { ssr: false },
);

export default function SwapPage() {
  return (
    <div className="mx-auto flex max-w-[575px] flex-col gap-3 px-6 pt-3 pb-10">
      <Suspense fallback={null}>
        <UrlSync />
      </Suspense>
      <SwapPanel />
      <GlobePanel />
    </div>
  );
}
