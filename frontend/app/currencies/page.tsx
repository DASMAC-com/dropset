import type { Metadata } from "next";
import { MobileSwapRedirect } from "@/components/chrome/MobileSwapRedirect";
import { CurrenciesView } from "@/components/picker/CurrenciesView";

export const metadata: Metadata = {
  title: "Currencies | Dropset",
};

export default function CurrenciesPage() {
  return (
    <>
      <MobileSwapRedirect />
      <CurrenciesView />
    </>
  );
}
