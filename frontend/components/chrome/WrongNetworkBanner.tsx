"use client";

import { CLUSTER } from "@/lib/env";
import { useClusterStatus } from "@/lib/hooks/useClusterStatus";

// Full-width notice under the header once every RPC endpoint has answered
// with a chain other than the one the build targets. The gate already refuses
// every chain read and the submit CTAs are held; this says why. A misconfigured
// deploy is not the user's to fix, so the copy only tells them to wait it out.
export function WrongNetworkBanner() {
  if (useClusterStatus() !== "mismatch") return null;
  return (
    <div
      role="alert"
      className="border-amber-400/40 border-b bg-amber-400/10 px-4 py-2 text-center text-amber-300 text-sm"
    >
      <span className="font-medium">Wrong network.</span> This site&apos;s
      connection is not serving Solana {CLUSTER}, so balances and swaps are
      switched off. Nothing has been sent; try again later.
    </div>
  );
}
