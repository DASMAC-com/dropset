import { defineConfig } from "vitest/config";

// The frontend's unit runner. Deliberately scoped to pure logic — the
// price/decimal conversions, the icon-source resolution — because that is the
// class of bug that has actually shipped here, and because a Rust twin of the
// same price logic already gets covered, leaving the TS fork as the only
// untested side of a cross-language pair.
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
    include: ["**/*.test.ts"],
    // next's own build output and the mirrored assets have nothing to run.
    exclude: ["node_modules/**", ".next/**"],
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
