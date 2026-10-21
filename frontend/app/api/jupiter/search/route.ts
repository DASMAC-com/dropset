import { forward, JUPITER_SEARCH_UPSTREAM } from "@/lib/server/apiProxy";

// Jupiter token search with the server-held API key; see
// lib/server/apiProxy.ts.
export const GET = (req: Request): Promise<Response> =>
  forward(JUPITER_SEARCH_UPSTREAM, req);
