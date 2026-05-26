"use client";

import { motion } from "motion/react";
import { useState } from "react";
import { emit, useAppEvent } from "@/lib/events";
import { ArrowUpDown } from "./icons";

// 540° = 1.5 turns. Picked over a plain 360° so the rotation reads as a
// confident "flip + a bit more" rather than a literal half-turn that
// could look indecisive. Same value is reused for the hover hint.
const SPIN_DEGREES = 540;
// Stiff/fast spring tuned to settle inside the user's eye-track window —
// looser values let the arrow keep wobbling after the from/to swap has
// already finished re-rendering, which reads as laggy UI.
const SPIN_SPRING = { type: "spring", stiffness: 800, damping: 70 } as const;

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
      animate={{
        rotate: eventSpins * SPIN_DEGREES + (hovering ? SPIN_DEGREES : 0),
      }}
      transition={SPIN_SPRING}
      className="flex h-10 w-10 items-center justify-center rounded-full border border-border bg-background text-muted-fg shadow-sm transition-colors hover:border-accent hover:text-accent"
      aria-label="Swap sell and buy sides"
    >
      <ArrowUpDown size={19} strokeWidth={2} />
    </motion.button>
  );
}
