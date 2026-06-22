// cspell:word extensionless
import { defineConfig } from "tsup";

// The published package must ship compiled `.js` + `.d.ts` — consumers
// won't run `tsx`. A plain `tsc` build can't help: the Codama-generated
// tree under `src/generated` uses extensionless, directory-style imports
// (`export * from './accounts'`), the bundler convention, which `tsc`
// emits verbatim and Node ESM then can't resolve. tsup resolves and
// bundles those internal imports while leaving `@solana/kit` (a declared
// dependency) external, so the dist is Node-resolvable ESM.
//
// Two entry points mirror the package `exports` map: the root re-export
// and the `./generated` subpath, emitted to `dist/index.*` and
// `dist/generated/index.*` to match `publishConfig`.
export default defineConfig({
  clean: true,
  dts: true,
  entry: {
    index: "src/index.ts",
    "generated/index": "src/generated/index.ts",
  },
  format: ["esm"],
  outDir: "dist",
  // Neutral platform: the SDK is isomorphic (frontend + Node bots/routers),
  // so don't bake in Node- or browser-specific resolution or globals.
  platform: "neutral",
  // No source maps: `files` ships dist/ only, so maps would point at
  // `src/` that isn't in the tarball — dead weight for consumers.
  sourcemap: false,
  target: "es2022",
  tsconfig: "tsconfig.build.json",
});
