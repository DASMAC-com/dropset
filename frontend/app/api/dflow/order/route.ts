import { forward } from "@/lib/server/apiProxy";
import { DFLOW_ORDER_ROUTE } from "@/lib/server/proxyQuery";

// DFlow /order with the server-held API key; see lib/server/apiProxy.ts and
// the query check in lib/server/proxyQuery.ts.
export const GET = (req: Request): Promise<Response> =>
  forward(DFLOW_ORDER_ROUTE, req);
