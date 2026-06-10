"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";

// The currencies and vaults tables are wide, desktop-only views. On phones
// we send visitors to the swap page instead — including anyone who hard-links
// straight to /currencies or /vaults. The 767.98px ceiling is the exact
// complement of Tailwind's `md` breakpoint (min-width: 768px), which the
// header uses to hide these nav links, so navigation and routing stay in
// lockstep with no dead band between them. Rendered by the currencies/vaults
// pages; returns nothing.
export function MobileSwapRedirect() {
  const router = useRouter();
  useEffect(() => {
    const mq = window.matchMedia("(max-width: 767.98px)");
    const redirectIfMobile = () => {
      if (mq.matches) router.replace("/swap");
    };
    redirectIfMobile();
    // Also fire if the viewport shrinks below `md` while the page is open
    // (desktop → narrow resize, or a rotate on a tablet).
    mq.addEventListener("change", redirectIfMobile);
    return () => mq.removeEventListener("change", redirectIfMobile);
  }, [router]);
  return null;
}
