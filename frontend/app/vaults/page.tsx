import type { Metadata } from "next";
import { MobileSwapRedirect } from "@/components/chrome/MobileSwapRedirect";
import { VaultsView } from "@/components/vaults/VaultsView";

export const metadata: Metadata = {
  title: "Vaults | Dropset",
};

export default function VaultsPage() {
  return (
    <>
      <MobileSwapRedirect />
      <VaultsView />
    </>
  );
}
