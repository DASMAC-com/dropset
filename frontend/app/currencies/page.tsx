import type { Metadata } from "next";
import { CurrenciesView } from "@/components/CurrenciesView";

export const metadata: Metadata = {
  title: "Currencies | Dropset",
};

export default function CurrenciesPage() {
  return <CurrenciesView />;
}
