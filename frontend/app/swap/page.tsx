import { Suspense } from "react";
import { GlobePanel } from "@/components/GlobePanel";
import { SwapPanel } from "@/components/SwapPanel";
import { UrlSync } from "@/components/UrlSync";

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
