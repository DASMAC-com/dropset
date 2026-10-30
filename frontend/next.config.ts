// cspell:word transpiles
import type { NextConfig } from "next";
import { proxyFlags } from "./lib/server/apiProxy";
import { securityHeaders } from "./lib/server/securityHeaders";

const nextConfig: NextConfig = {
  // Whether the browser calls the keyed route handlers or the public endpoints
  // directly, derived from the server-only keys without exposing them.
  env: proxyFlags(),
  // Framing, sniffing and CSP headers on every route; see securityHeaders.ts.
  headers: async () => [
    {
      source: "/:path*",
      headers: securityHeaders(process.env, {
        dev: process.env.NODE_ENV === "development",
      }),
    },
  ],
  reactCompiler: true,
  typedRoutes: true,
  transpilePackages: [
    "react-globe.gl",
    "globe.gl",
    "three",
    "react-kapsule",
    // The workspace SDK serves raw TS to consumers (its build step emits
    // dist/ only for the published npm package); Next transpiles the source.
    "@dropset/sdk",
  ],
};

export default nextConfig;
