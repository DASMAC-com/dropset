import type { Metadata } from "next";
import { LiquidityView } from "@/components/liquidity/LiquidityView";

export const metadata: Metadata = {
  title: "Liquidity | Dropset",
};

export default function LiquidityPage() {
  return <LiquidityView />;
}
