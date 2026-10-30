// cspell:word clickjack
// The response headers every page carries, set by next.config.ts.
//
// Framing, MIME sniffing and the referrer are enforced: the site may never be
// framed (a hostile page could otherwise clickjack the picker or the swap
// CTA), and nothing it serves is sniffed into another type.
//
// The source allowlist ships report-only. A violation is logged to the browser
// console without blocking anything, so the wallet connectors (MetaMask
// Connect's relay, the Android Mobile Wallet Adapter's local socket) and the
// globe can be checked against it on a real deploy before it is enforced by
// moving it into the enforced policy. Next's own hydration scripts are inline,
// so scripts need 'unsafe-inline' until the app adopts per-request nonces.

import {
  DFLOW_ORDER_URLS,
  DFLOW_QUOTE_URLS,
  JUPITER_SEARCH_URLS,
} from "../upstreams";

export type Header = { key: string; value: string };

// The browser-visible endpoints next.config.ts can see: the RPC and WS pair,
// the optional primary pair, and any explicit API override. A relative
// override is a route on the app's own origin, already 'self'.
const ENDPOINT_VARS = [
  "NEXT_PUBLIC_RPC_URL",
  "NEXT_PUBLIC_WS_URL",
  "NEXT_PUBLIC_PRIMARY_RPC_URL",
  "NEXT_PUBLIC_PRIMARY_WS_URL",
  "NEXT_PUBLIC_DFLOW_QUOTE_URL",
  "NEXT_PUBLIC_DFLOW_ORDER_URL",
  "NEXT_PUBLIC_JUPITER_SEARCH_URL",
];

// Scheme, host and port only: a provider's key rides in the path or query,
// and a CSP source never needs either.
const sourceOf = (url: string): string | null => {
  try {
    const { protocol, host } = new URL(url);
    return `${protocol}//${host}`;
  } catch {
    return null;
  }
};

// Every origin the browser may fetch or open a socket to, besides its own.
// The keyless API hosts are listed whether or not a key is set: without one
// the browser calls them directly, and with one it calls the app's route.
export function connectSources(
  env: Record<string, string | undefined>,
): string[] {
  const urls = [
    ...ENDPOINT_VARS.map((name) => env[name]?.trim()),
    DFLOW_QUOTE_URLS.keyless,
    DFLOW_ORDER_URLS.keyless,
    JUPITER_SEARCH_URLS.keyless,
  ];
  const sources = urls.flatMap((url) => {
    const source = url ? sourceOf(url) : null;
    return source ? [source] : [];
  });
  return [...new Set(sources)];
}

export function securityHeaders(
  env: Record<string, string | undefined>,
  { dev }: { dev: boolean },
): Header[] {
  const baseline = [
    "default-src 'self'",
    // 'wasm-unsafe-eval' for the SDK's WebAssembly; dev's React refresh
    // also evaluates strings.
    `script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'${dev ? " 'unsafe-eval'" : ""}`,
    "style-src 'self' 'unsafe-inline'",
    // Token icons come from each issuer's own host.
    "img-src 'self' data: blob: https:",
    "font-src 'self' data:",
    `connect-src 'self' ${connectSources(env).join(" ")}`,
    "worker-src 'self' blob:",
    "frame-ancestors 'none'",
  ];
  return [
    {
      key: "Content-Security-Policy",
      value: "frame-ancestors 'none'; object-src 'none'; base-uri 'self'",
    },
    { key: "Content-Security-Policy-Report-Only", value: baseline.join("; ") },
    { key: "X-Frame-Options", value: "DENY" },
    { key: "X-Content-Type-Options", value: "nosniff" },
    { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
  ];
}
