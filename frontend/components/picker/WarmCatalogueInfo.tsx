"use client";

import { useCatalogueInfoRefresh } from "@/lib/hooks/useTokenInfo";

// Warms the catalogue's Jupiter prices and volumes (the rows' volume sort,
// USD readouts and liquidity badges) while a picker is open. Render it
// inside the dialog content, which is unmounted while closed, so the swap
// page's cold load pays for none of it.
export function WarmCatalogueInfo() {
  useCatalogueInfoRefresh();
  return null;
}
