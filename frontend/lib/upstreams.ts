// The public endpoint URLs of the external APIs, in one place. Client-safe:
// only URL strings live here, no key material. lib/server/apiProxy.ts builds
// its keyed upstreams from these, and lib/env.ts uses the keyless ones as the
// browser's default direct endpoints, so unless a NEXT_PUBLIC_*_URL override
// is set, the browser and the server's keyless fallback name the same hosts.

export type UpstreamUrls = {
  // Where a request goes with the server's key, and where it goes without one.
  keyed: string;
  keyless: string;
};

// DFlow's production Trade API takes the key; the developer endpoint is the
// keyless one (https://pond.dflow.net, "Endpoints").
export const DFLOW_QUOTE_URLS: UpstreamUrls = {
  keyed: "https://quote-api.dflow.net/quote",
  keyless: "https://dev-quote-api.dflow.net/quote",
};
export const DFLOW_ORDER_URLS: UpstreamUrls = {
  keyed: "https://quote-api.dflow.net/order",
  keyless: "https://dev-quote-api.dflow.net/order",
};
// Jupiter's keyed host serves the same Tokens API path as lite-api.
export const JUPITER_SEARCH_URLS: UpstreamUrls = {
  keyed: "https://api.jup.ag/tokens/v2/search",
  keyless: "https://lite-api.jup.ag/tokens/v2/search",
};
