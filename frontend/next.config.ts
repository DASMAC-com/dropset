// cspell:word transpiles
import type { NextConfig } from "next";

const nextConfig: NextConfig = {
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
