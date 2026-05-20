"use client";

import { motion } from "motion/react";
import { useState } from "react";
import { emit, useAppEvent } from "@/lib/events";
import { ArrowUpDown } from "./icons";

export function SwapArrowButton() {
  const [hovering, setHovering] = useState(false);
  const [eventSpins, setEventSpins] = useState(0);
  // The actual swap is handled by SwapPanel's swapSides listener (which has
  // access to the quote and can promote the output amount). This component
  // only emits the event and animates the spin.
  useAppEvent("swapSides", () => setEventSpins((n) => n + 1));
  return (
    <motion.button
      type="button"
      onClick={() => emit("swapSides")}
      onHoverStart={() => setHovering(true)}
      onHoverEnd={() => setHovering(false)}
      animate={{ rotate: eventSpins * 540 + (hovering ? 540 : 0) }}
      transition={{ type: "spring", stiffness: 800, damping: 70 }}
      className="flex h-10 w-10 items-center justify-center rounded-full border border-border bg-background text-muted-fg shadow-sm transition-colors hover:border-accent hover:text-accent"
      aria-label="Swap sell and buy sides"
    >
      <ArrowUpDown size={19} strokeWidth={2} />
    </motion.button>
  );
}
