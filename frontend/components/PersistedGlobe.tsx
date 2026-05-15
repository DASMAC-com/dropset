"use client";

import { usePathname } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { GlobePanel } from "./GlobePanel";

// Mount the GlobePanel exactly once at the root layout so the three.js scene
// survives client-side navigations. On the swap page, the wrapper is
// position:fixed and snapped over a #globe-slot placeholder div. On other
// pages, the wrapper is parked off-screen but stays mounted (and rotating).

type Rect = { top: number; left: number; width: number; height: number };

const FALLBACK_RECT: Rect = { top: 0, left: 0, width: 527, height: 448 };

export function PersistedGlobe() {
  const pathname = usePathname();
  const onSwap = pathname === "/swap";
  const [rect, setRect] = useState<Rect | null>(null);
  const lastRectRef = useRef<Rect | null>(null);

  useEffect(() => {
    if (!onSwap) return;
    const slot = document.getElementById("globe-slot");
    if (!slot) return;
    const update = () => {
      const r = slot.getBoundingClientRect();
      const next = {
        top: r.top,
        left: r.left,
        width: r.width,
        height: r.height,
      };
      lastRectRef.current = next;
      setRect(next);
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(slot);
    window.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    return () => {
      ro.disconnect();
      window.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [onSwap]);

  const parked = lastRectRef.current ?? FALLBACK_RECT;
  const style: React.CSSProperties = onSwap && rect
    ? {
        position: "fixed",
        top: rect.top,
        left: rect.left,
        width: rect.width,
        height: rect.height,
        zIndex: 1,
      }
    : {
        position: "fixed",
        top: 0,
        left: -99999,
        width: parked.width,
        height: parked.height,
        pointerEvents: "none",
        zIndex: -1,
      };

  return (
    <div aria-hidden={!onSwap} style={style}>
      <GlobePanel />
    </div>
  );
}
