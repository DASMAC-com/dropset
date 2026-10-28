import { forward } from "@/lib/server/apiProxy";
import { DFLOW_QUOTE_ROUTE } from "@/lib/server/proxyQuery";

// DFlow /quote with the server-held API key; see lib/server/apiProxy.ts and
// the query check in lib/server/proxyQuery.ts.
export const GET = (req: Request): Promise<Response> =>
  forward(DFLOW_QUOTE_ROUTE, req);
