// Server-only: imported by the route handlers under app/api/ and by
// next.config.ts, never by a client module, because it reads the third-party
// API keys. A key here is a server-only variable (never NEXT_PUBLIC_), so it
// stays out of the browser bundle; the browser calls the relative route and
// the handler adds the key on the way upstream.
//
// Every key is optional. Unset, the handler forwards keyless to the public
// endpoint, and next.config.ts leaves the browser calling that endpoint
// directly (see `proxyFlags`), so a deploy with no key behaves as it did
// before the proxy existed.

import {
  DFLOW_ORDER_URLS,
  DFLOW_QUOTE_URLS,
  JUPITER_SEARCH_URLS,
  type UpstreamUrls,
} from "../upstreams";
import { type ClientLimiter, clientAddress } from "./clientLimit";

// An unresolved 1Password reference. Locally the key's variable holds one in
// the git-ignored .env.local, and `make frontend` resolves it through `op run`;
// a bare `pnpm dev` hands the reference through literally, which is a
// configuration error, not a key.
const OP_REFERENCE = /^op:\/\//;

export type KeyedUpstream = {
  // The server-only variable holding the key.
  keyVar: string;
  // Where a request goes with the key, and where it goes without one.
  keyedUrl: string;
  keylessUrl: string;
};

// The URLs live in lib/upstreams.ts, shared with the browser's keyless
// defaults in lib/env.ts.
const keyed = (keyVar: string, urls: UpstreamUrls): KeyedUpstream => ({
  keyVar,
  keyedUrl: urls.keyed,
  keylessUrl: urls.keyless,
});
export const DFLOW_QUOTE_UPSTREAM = keyed("DFLOW_API_KEY", DFLOW_QUOTE_URLS);
export const DFLOW_ORDER_UPSTREAM = keyed("DFLOW_API_KEY", DFLOW_ORDER_URLS);
export const JUPITER_SEARCH_UPSTREAM = keyed(
  "JUPITER_API_KEY",
  JUPITER_SEARCH_URLS,
);

export type Env = Record<string, string | undefined>;

// A route's query check: the query string to send upstream, rebuilt from the
// parameters the route allows, or the reason to refuse the request.
export type QueryCheck =
  | { ok: true; search: string }
  | { ok: false; msg: string };
export type QueryValidator = (
  params: URLSearchParams,
  env: Env,
) => Promise<QueryCheck>;

// One route handler: the upstream it forwards to, the check every query
// passes before the key is attached, and optionally a per-caller limit. The
// validators and limits live in lib/server/proxyQuery.ts, apart from this
// module, which next.config.ts imports.
export type ProxyRoute = {
  upstream: KeyedUpstream;
  validate: QueryValidator;
  limiter?: ClientLimiter;
};

// The key, or null when unset. Throws on an unresolved reference, naming the
// variable and never its value.
export function serverKey(name: string, env: Env = process.env): string | null {
  const value = env[name]?.trim();
  if (!value) return null;
  if (OP_REFERENCE.test(value)) {
    throw new Error(
      `${name} holds an unresolved 1Password reference; run the frontend under op run (make frontend)`,
    );
  }
  return value;
}

// Response headers passed back to the browser. The rate-limit pair feeds
// lib/dflow/rateLimitBudget.ts, which reads them when it can see them; a
// same-origin route makes them visible, where the cross-origin call hid them.
// The content type is not among them: every response is JSON (see `forward`).
const FORWARDED_HEADERS = [
  "retry-after",
  "x-ratelimit-limit",
  "x-ratelimit-remaining",
  "x-ratelimit-reset",
];

const JSON_TYPE = /^application\/json\b/i;
const NULL_BODY_STATUSES = new Set([204, 205, 304]);

// A failure of the proxy's own: a `{ msg }` body, the shape DFlow's errors
// take.
const fail = (
  status: number,
  msg: string,
  headers: Record<string, string> = {},
): Response =>
  Response.json(
    { msg },
    { status, headers: { ...headers, "x-content-type-options": "nosniff" } },
  );

// Forward a GET to the upstream with the key when one is set, after the
// route's query check: the query sent upstream is the one the check rebuilt,
// never the caller's raw string, and a refused query is a 400 that never
// reaches the upstream. The upstream's status and JSON body pass through as
// they are, so a 429 or an API error reaches the caller exactly as a direct
// call would.
//
// Served on the app's own origin, so the response is always JSON with
// nosniff: a non-JSON upstream body (an edge or error page) is replaced by a
// `{ msg }` under the same status rather than served as whatever type it
// claims. A browser request from another site (`Sec-Fetch-Site` other than
// same-origin) is refused, so no other page can spend the key through its
// visitors' browsers; a non-browser client omits the header and is not
// stopped by this. What bounds that client is the route's limiter, when it
// has one: a caller over it gets a 429 with `retry-after` before its query
// is checked or the key read. HEAD, which Next answers through the GET
// handler, is refused too, since it would spend quota for nothing.
export async function forward(
  route: ProxyRoute,
  req: Request,
  env: Env = process.env,
): Promise<Response> {
  if (req.method !== "GET") return fail(405, "Only GET is proxied");
  const site = req.headers.get("sec-fetch-site");
  if (site !== null && site !== "same-origin") {
    return fail(403, "Cross-site requests are not proxied");
  }

  const wait = route.limiter?.take(clientAddress(req)) ?? null;
  if (wait !== null) {
    return fail(429, `Too many requests; retry in ${wait}s`, {
      "retry-after": String(wait),
    });
  }

  const { upstream } = route;
  const check = await route.validate(new URL(req.url).searchParams, env);
  if (!check.ok) return fail(400, check.msg);

  let key: string | null;
  try {
    key = serverKey(upstream.keyVar, env);
  } catch (e) {
    return fail(500, e instanceof Error ? e.message : String(e));
  }

  const url = new URL(key ? upstream.keyedUrl : upstream.keylessUrl);
  url.search = check.search;
  const headers: Record<string, string> = { accept: "application/json" };
  if (key) headers["x-api-key"] = key;

  // A redirect is refused rather than followed: fetch would re-send the
  // custom key header to whatever host the redirect names, and neither API
  // redirects a JSON call.
  let res: Response;
  try {
    res = await fetch(url, { headers, redirect: "error", signal: req.signal });
  } catch {
    return fail(
      502,
      `Network error or refused redirect reaching ${url.hostname}`,
    );
  }

  const out = new Headers({
    "content-type": "application/json",
    "x-content-type-options": "nosniff",
  });
  for (const name of FORWARDED_HEADERS) {
    const value = res.headers.get(name);
    if (value !== null) out.set(name, value);
  }
  // A status that cannot carry a body (204, 304, …) passes through bodiless.
  if (NULL_BODY_STATUSES.has(res.status)) {
    await res.body?.cancel().catch(() => {});
    return new Response(null, { status: res.status, headers: out });
  }
  if (!JSON_TYPE.test(res.headers.get("content-type") ?? "")) {
    // Best-effort: a stream that already errored rejects the cancel. A 2xx
    // becomes a 502, so a caller checking `res.ok` never parses the `{ msg }`
    // as a result; an error status is kept, so a 429 still reads as one.
    await res.body?.cancel().catch(() => {});
    return new Response(
      JSON.stringify({
        msg: `${url.hostname} answered ${res.status} with a non-JSON body`,
      }),
      { status: res.ok ? 502 : res.status, headers: out },
    );
  }
  return new Response(res.body, { status: res.status, headers: out });
}

// The build-time switches next.config.ts bakes into the browser bundle: "1"
// when the key is set and the browser should call the route, "" when it
// should keep calling the public endpoint directly. Keyless traffic stays in
// the browser on purpose: through the proxy every user would share the
// server's one IP and its one per-IP quota. Throws (failing the build or dev
// start) on an unresolved reference, and logs each upstream's mode by
// hostname on every config load (Next loads the config more than once).
export function proxyFlags(env: Env = process.env): {
  NEXT_PUBLIC_DFLOW_PROXY: string;
  NEXT_PUBLIC_JUPITER_PROXY: string;
} {
  const flag = (label: string, upstream: KeyedUpstream): string => {
    const keyed = serverKey(upstream.keyVar, env) !== null;
    const host = new URL(keyed ? upstream.keyedUrl : upstream.keylessUrl)
      .hostname;
    console.log(
      `${label}: ${keyed ? `keyed, via the route handler to ${host}` : `keyless, browser direct to ${host}`}`,
    );
    return keyed ? "1" : "";
  };
  return {
    NEXT_PUBLIC_DFLOW_PROXY: flag("DFlow", DFLOW_QUOTE_UPSTREAM),
    NEXT_PUBLIC_JUPITER_PROXY: flag("Jupiter", JUPITER_SEARCH_UPSTREAM),
  };
}
