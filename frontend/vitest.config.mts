import { defineConfig } from "vitest/config";

// The frontend's unit runner. Deliberately scoped to pure logic — the
// price/decimal conversions, the icon-source resolution — because that is the
// class of bug that has actually shipped here. Part of that price logic has a
// Rust counterpart that is covered (util/src/decimals.rs); the rest of it, and
// all of the icon resolution, is covered nowhere else.
//
// It is NOT a hook or integration harness: the availability-latch class of bug
// needs a live validator, which is a separate build. Adding jsdom and
// @testing-library here later is the natural extension.
//
// `.mts` because the config uses ESM syntax and package.json has no
// `"type": "module"` — a plain `.ts` here is loaded as CommonJS.
export default defineConfig({
  resolve: {
    // Resolves the `@/*` alias from tsconfig.json so tests import modules the
    // same way the app does. Native since Vite 7; the vite-tsconfig-paths
    // plugin is no longer needed for this.
    tsconfigPaths: true,
  },
  test: {
    // `.mjs` is here for the build scripts and `.tsx` so the first component
    // test does not silently match nothing; a bare `**/*.test.ts` would.
    include: ["**/*.test.{ts,tsx,mjs}"],
    // This replaces vitest's default exclude list rather than extending it,
    // so anything that must stay unmatched has to be named here.
    exclude: ["node_modules/**", ".next/**", "dist/**", "coverage/**"],
    // Modules under lib/ read their config through lib/env, which throws on a
    // missing value for any of its three `required` vars. Unit tests must not
    // depend on a developer's .env.local (CI has none), and nothing here makes
    // a network call, so pin syntactically valid placeholders rather than
    // reachable endpoints. Every other var in lib/env has a default.
    env: {
      NEXT_PUBLIC_GET_MULTIPLE_ACCOUNTS_BATCH_SIZE: "10",
      NEXT_PUBLIC_RPC_URL: "http://127.0.0.1:8899",
      NEXT_PUBLIC_WS_URL: "ws://127.0.0.1:8900",
    },
  },
});
