/** @type {import('next').NextConfig} */
const nextConfig = {
  experimental: {
    /**
     * Keep the browser stack out of the bundler.
     *
     * `@sparticuz/chromium` carries a brotli-compressed Chromium it unpacks at
     * runtime, and `puppeteer-core` resolves parts of itself dynamically.
     * Bundling either one rewrites the paths they rely on, so the export route
     * fails on a deployment while working locally. Marking them external
     * leaves both as plain `require`s from `node_modules`.
     */
    serverComponentsExternalPackages: ["@sparticuz/chromium", "puppeteer-core"],
  },
};

export default nextConfig;
