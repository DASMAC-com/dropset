import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactCompiler: true,
  typedRoutes: true,
  transpilePackages: [
    "react-globe.gl",
    "globe.gl",
    "three",
    "react-kapsule",
    // Workspace SDK ships raw TS (no build step); Next transpiles it.
    "@dropset/sdk",
  ],
};

export default nextConfig;
