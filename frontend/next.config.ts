import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactCompiler: true,
  typedRoutes: true,
  transpilePackages: ["react-globe.gl", "globe.gl", "three", "react-kapsule"],
};

export default nextConfig;
