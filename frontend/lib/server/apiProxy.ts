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

// DFlow's production Trade API takes the key; the developer endpoint is the
// keyless one the browser called directly before (https://pond.dflow.net,
// "Endpoints").
export const DFLOW_QUOTE_UPSTREAM: KeyedUpstream = {
  keyVar: "DFLOW_API_KEY",
  keyedUrl: "https://quote-api.dflow.net/quote",
  keylessUrl: "https://dev-quote-api.dflow.net/quote",
};
export const DFLOW_ORDER_UPSTREAM: KeyedUpstream = {
  keyVar: "DFLOW_API_KEY",
  keyedUrl: "https://quote-api.dflow.net/order",
  keylessUrl: "https://dev-quote-api.dflow.net/order",
};
// Jupiter's keyed host serves the same Tokens API path as lite-api.
export const JUPITER_SEARCH_UPSTREAM: KeyedUpstream = {
  keyVar: "JUPITER_API_KEY",
  keyedUrl: "https://api.jup.ag/tokens/v2/search",
  keylessUrl: "https://lite-api.jup.ag/tokens/v2/search",
};

type Env = Record<string, string | undefined>;

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
const FORWARDED_HEADERS = [
  "content-type",
  "retry-after",
  "x-ratelimit-limit",
  "x-ratelimit-remaining",
  "x-ratelimit-reset",
];

// Forward a GET to the upstream, query string unchanged, with the key when one
// is set. The upstream's status and body pass through as they are, so a 429
// or an API error reaches the caller exactly as a direct call would. A failure
// of the proxy's own is a `{ msg }` body, the shape DFlow's errors take.
export async function forward(
  upstream: KeyedUpstream,
  req: Request,
  env: Env = process.env,
): Promise<Response> {
  let key: string | null;
  try {
    key = serverKey(upstream.keyVar, env);
  } catch (e) {
    return Response.json(
      { msg: e instanceof Error ? e.message : String(e) },
      { status: 500 },
    );
  }

  const url = new URL(key ? upstream.keyedUrl : upstream.keylessUrl);
  url.search = new URL(req.url).search;
  const headers: Record<string, string> = { accept: "application/json" };
  if (key) headers["x-api-key"] = key;

  // A redirect is refused rather than followed: fetch would re-send the
  // custom key header to whatever host the redirect names, and neither API
  // redirects a JSON call.
  let res: Response;
  try {
    res = await fetch(url, { headers, redirect: "error", signal: req.signal });
  } catch {
    return Response.json(
      { msg: `Network error reaching ${url.hostname}` },
      { status: 502 },
    );
  }

  const out = new Headers();
  for (const name of FORWARDED_HEADERS) {
    const value = res.headers.get(name);
    if (value !== null) out.set(name, value);
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
