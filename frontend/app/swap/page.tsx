import { Suspense } from "react";
import { GlobePanel } from "@/components/GlobePanel";
import { SwapPanel } from "@/components/SwapPanel";
import { SwapStateInitializer } from "@/components/SwapStateInitializer";
import { UrlSync } from "@/components/UrlSync";

// Read ?from / ?to here, in the server component, so the URL-derived pair is
// passed down to SwapStateInitializer as plain strings. The initializer's
// lazy state initializer runs synchronously during render — on both SSR and
// client hydration — and seeds the per-tree Zustand store before sibling
// components below read it. Result: the very first paint reflects the URL's
// pair (USDC/EURC only when no slugs are supplied), with no effect-driven
// stitch and no hydration mismatch.
export default async function SwapPage({
  searchParams,
}: {
  searchParams: Promise<{ from?: string; to?: string }>;
}) {
  const params = await searchParams;
  return (
    <div className="mx-auto flex max-w-[575px] flex-col gap-3 px-6 pt-3 pb-10">
      <SwapStateInitializer fromSlug={params.from} toSlug={params.to} />
      <Suspense fallback={null}>
        <UrlSync />
      </Suspense>
      <SwapPanel />
      <GlobePanel />
    </div>
  );
}
