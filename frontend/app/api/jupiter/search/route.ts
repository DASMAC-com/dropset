import { forward } from "@/lib/server/apiProxy";
import { JUPITER_SEARCH_ROUTE } from "@/lib/server/proxyQuery";

// Jupiter token search with the server-held API key; see
// lib/server/apiProxy.ts and the query check in lib/server/proxyQuery.ts.
export const GET = (req: Request): Promise<Response> =>
  forward(JUPITER_SEARCH_ROUTE, req);
