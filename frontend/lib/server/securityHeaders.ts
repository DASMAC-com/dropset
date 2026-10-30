// cspell:word clickjack
// The response headers every page carries, set by next.config.ts.
//
// Everything here is enforced: the site may never be framed (a hostile page
// could otherwise clickjack the picker or the swap CTA), nothing it serves is
// sniffed into another type, no script loads from a foreign host, and fetches
// and sockets reach only the sources listed. A connector that breaks under
// the policy is a missing source, added here, never a reason to widen a
// directive. Next's own hydration scripts are inline, so scripts need
// 'unsafe-inline' until the app adopts per-request nonces; until then the
// policy does not stop an injected inline script sending data out (an image
// request to any https: host, or a message on the MetaMask relay).

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

// The origins the wallet connectors' SDKs open themselves, read from the
// SDKs' source; each must also survive a connect on a deploy.
export const CONNECTOR_SOURCES = [
  // MetaMask Connect: the relay its QR and deep-link sessions ride.
  "wss://mm-sdk-relay.api.cx.metamask.io",
  // MetaMask Connect: the connection counters `analytics` enables.
  "https://mm-sdk-analytics.api.cx.metamask.io",
  // Android Mobile Wallet Adapter: the wallet app's local association
  // socket, on a port it picks per session.
  "ws://localhost:*",
];

// The frames the connectors' SDKs open: MetaMask's onboarding forwarder, a
// hidden frame its extension-install flow talks to.
export const CONNECTOR_FRAMES = ["https://fwd.metamask.io"];

export function securityHeaders(
  env: Record<string, string | undefined>,
  { dev }: { dev: boolean },
): Header[] {
  const policy = [
    "default-src 'self'",
    // 'wasm-unsafe-eval' for the SDK's WebAssembly; dev's React refresh
    // also evaluates strings.
    `script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'${dev ? " 'unsafe-eval'" : ""}`,
    "style-src 'self' 'unsafe-inline'",
    // Token icons come from each issuer's own host, which no list can name
    // ahead of time; an image runs no script, though a request for one can
    // carry data out (see the top of this file).
    "img-src 'self' data: blob: https:",
    "font-src 'self' data:",
    `connect-src 'self' ${[...connectSources(env), ...CONNECTOR_SOURCES].join(" ")}`,
    `frame-src ${CONNECTOR_FRAMES.join(" ")}`,
    "worker-src 'self' blob:",
    "object-src 'none'",
    "base-uri 'self'",
    // The app posts no form; injected markup may not post one elsewhere.
    "form-action 'self'",
    "frame-ancestors 'none'",
  ];
  return [
    { key: "Content-Security-Policy", value: policy.join("; ") },
    { key: "X-Frame-Options", value: "DENY" },
    { key: "X-Content-Type-Options", value: "nosniff" },
    { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
  ];
}
