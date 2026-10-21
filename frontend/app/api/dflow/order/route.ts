import { DFLOW_ORDER_UPSTREAM, forward } from "@/lib/server/apiProxy";

// DFlow /order with the server-held API key; see lib/server/apiProxy.ts.
export const GET = (req: Request): Promise<Response> =>
  forward(DFLOW_ORDER_UPSTREAM, req);
