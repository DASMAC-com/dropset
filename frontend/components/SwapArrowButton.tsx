"use client";

import { motion } from "motion/react";
import { useState } from "react";
import { useAppEvent } from "@/lib/events";
import { useSwapStore } from "@/lib/store";
import { ArrowUpDown } from "./icons";

export function SwapArrowButton() {
  const [hovering, setHovering] = useState(false);
  const swapSides = useSwapStore((s) => s.swapSides);
  useAppEvent("swapSides", swapSides);
  return (
    <motion.button
      type="button"
      onClick={swapSides}
      onHoverStart={() => setHovering(true)}
      onHoverEnd={() => setHovering(false)}
      animate={hovering ? { rotate: 540 } : { rotate: 0 }}
      transition={{ type: "spring", stiffness: 800, damping: 70 }}
      className="flex h-8 w-8 items-center justify-center rounded-full border border-border bg-background text-muted-fg shadow-sm transition-colors hover:border-accent hover:text-accent"
      aria-label="Swap sell and buy sides"
    >
      <ArrowUpDown size={15} strokeWidth={2} />
    </motion.button>
  );
}
