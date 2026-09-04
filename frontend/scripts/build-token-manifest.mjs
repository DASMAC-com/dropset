// Write lib/data/icon-manifest.gen.json (symbol → /token-icons/<file>) from
// the icons committed under brand-assets/token-icons/. Replaces
// fetch-token-icons.mjs, which built the same manifest by fetching ~25
// third-party URLs on the build path.
//
// NO NETWORK ACCESS. That is the point: a third-party host can no longer
// fail a build, a dev server start, or the merge queue. The committed bytes
// are the source of truth and each token's `icon` URL in currencies.json is
// now the declared upstream source that audit-token-icons.mjs checks them
// against, out of band.
//
// The manifest is still generated rather than committed, so the committed
// bytes stay the single source of truth — the manifest cannot drift from the
// directory it is derived from. It stays gitignored, as
// frontend/.gitignore already has it.
//
// Pass --strict (CI does) to exit non-zero when a listed currency has no
// committed icon. Unlike the strict gate this replaces, that condition is
// purely LOCAL and deterministic: it reads a directory, dials nothing, and
// so cannot flake. It keeps the original gate's intent — a token whose
// artwork is missing blocks the merge — while dropping the third-party
// dependency that made the old gate a queue hazard. Off by default so
// `pnpm dev` still starts, degrading to the remote URL that currencies.ts
// keeps as a render-side fallback.
import { writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  committedIconFor,
  listCommitted,
  readTokens,
  URL_PREFIX,
} from "./token-icons-shared.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const manifestPath = resolve(here, "../lib/data/icon-manifest.gen.json");

const strict = process.argv.includes("--strict");
const tokens = readTokens();
const entries = listCommitted();

const manifest = {};
const missing = [];

for (const token of tokens) {
  const committed = committedIconFor(token.symbol, entries);
  if (!committed) {
    missing.push(`${token.symbol} (${token.mint})`);
    continue;
  }
  manifest[token.symbol] = `${URL_PREFIX}/${committed.filename}`;
}

// Written even when empty, so the static TS import in currencies.ts never
// breaks — the same invariant the fetch-based script maintained.
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

console.log(
  `Manifested ${Object.keys(manifest).length}/${tokens.length} token icons from brand-assets/token-icons.`,
);

if (missing.length) {
  console.warn(
    `  ${missing.length} listed currency(s) have no committed icon (will fall back to the remote URL):`,
  );
  for (const symbol of missing) console.warn(`  - ${symbol}`);
  console.warn(
    "  Fetch them with: pnpm --filter dropset-frontend audit-token-icons --write",
  );
}

if (strict && missing.length) {
  console.error(
    `\n--strict: ${missing.length}/${tokens.length} listed currency(s) have no committed token icon.`,
  );
  // Set the code rather than calling process.exit(): under CI both streams
  // are pipes and an explicit exit can truncate the reasons above.
  process.exitCode = 1;
}
