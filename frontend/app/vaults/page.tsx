import type { Metadata } from "next";
import { VaultsView } from "@/components/vaults/VaultsView";

export const metadata: Metadata = {
  title: "Vaults | Dropset",
};

export default function VaultsPage() {
  return <VaultsView />;
}
