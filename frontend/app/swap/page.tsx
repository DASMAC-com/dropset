import { Suspense } from "react";
import { SwapPanel } from "@/components/SwapPanel";
import { UrlSync } from "@/components/UrlSync";

export default function SwapPage() {
  return (
    <div className="mx-auto flex max-w-[575px] flex-col gap-3 px-6 pt-3 pb-10">
      <Suspense fallback={null}>
        <UrlSync />
      </Suspense>
      <SwapPanel />
      {/* Reserved space for the persistent globe (mounted once in layout). */}
      <div
        id="globe-slot"
        className="aspect-[1/0.85] max-h-[480px] min-h-[320px] w-full"
      />
    </div>
  );
}
