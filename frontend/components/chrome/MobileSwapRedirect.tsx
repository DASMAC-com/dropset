"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { isMobile } from "@/lib/ua";

// The currencies and vaults tables are wide, desktop-only views, so on actual
// mobile devices we send visitors straight to the swap page — including anyone
// who hard-links to /currencies or /vaults.
//
// This keys off the user-agent (isMobile), NOT the viewport width, and runs
// once on mount. Width-based detection meant resizing a desktop browser across
// the breakpoint redirected you mid-resize — jarring, and it stranded you on
// /swap when you only meant to shrink the window for a moment. A desktop
// browser is never "mobile" by UA no matter how narrow you drag it, so
// resizing is now a no-op; narrow desktop windows fall back to the
// `md:hidden` "best viewed wider" card the table pages render instead.
//
// isMobile() is client-only (false during SSR), which is fine here: this runs
// in an effect, after hydration, where navigator is available. Rendered by the
// currencies/vaults pages; renders nothing.
export function MobileSwapRedirect() {
  const router = useRouter();
  useEffect(() => {
    if (isMobile()) router.replace("/swap");
  }, [router]);
  return null;
}
