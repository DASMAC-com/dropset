import { DFLOW_QUOTE, forward } from "@/lib/server/apiProxy";

// DFlow /quote with the server-held API key; see lib/server/apiProxy.ts.
export const GET = (req: Request): Promise<Response> =>
  forward(DFLOW_QUOTE, req);
