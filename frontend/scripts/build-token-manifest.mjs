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
// One behavioral difference from the script this replaces: it wrote the image
// FILES into public/token-icons as well as the manifest, whereas this writes
// only the manifest — the files now arrive via copy-brand-assets.mjs, which
// runs in predev/prebuild but NOT postinstall. So after a bare `pnpm install`
// the manifest names 25 paths that are not on disk yet. Harmless in every real
// flow (`next dev` and `next build` both run their pre-hook first, `next start`
// follows a build, and vitest only ever reads the strings), but worth knowing
// before treating a post-install tree as servable.
//
// Pass --strict (CI does) to exit non-zero when a listed currency has no
// committed icon, or has MORE THAN ONE. Unlike the strict gate this replaces,
// either condition is
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
const conflicts = [];

for (const token of tokens) {
  // `committedIconFor` THROWS when a symbol has two committed icons, and this
  // is the build path — it runs in postinstall, predev and prebuild. Letting
  // that throw escape breaks `pnpm install`, `pnpm dev` and `pnpm build` with
  // an uncaught stack trace, and leaves the manifest unwritten, so on a fresh
  // clone (it is gitignored) currencies.ts's static import resolves to nothing
  // and the real cause surfaces far away. Measured: a duplicate produced
  // `Error: USDC has 2 committed icons (USDC.png, USDC.svg)` and exit 1 out of
  // a bare `node build-token-manifest.mjs`.
  //
  // That state is reachable from the workflow this pipeline itself prescribes:
  // `audit-token-icons.mjs --write` deliberately leaves the old file in place
  // when an issuer changes format, precisely so the duplicate is noticed. It
  // has to be noticed WITHOUT bricking the build, so the conflict degrades
  // here like a missing icon does and fails only under --strict.
  let committed;
  try {
    committed = committedIconFor(token.symbol, entries);
  } catch (err) {
    conflicts.push(`${token.symbol} (${token.mint}): ${err.message}`);
    continue;
  }
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

if (conflicts.length) {
  console.warn(
    `  ${conflicts.length} symbol(s) have MORE THAN ONE committed icon (will fall back to the remote URL):`,
  );
  for (const line of conflicts) console.warn(`  - ${line}`);
  console.warn(
    "  This is what an upstream format change leaves behind; delete the stale file.",
  );
}

if (strict && (missing.length || conflicts.length)) {
  // Only the non-zero clauses. Printing both unconditionally leads with a
  // literal `0/25` on a conflict-only failure, and this is the first line
  // someone reads when a REQUIRED gate fails.
  const reasons = [];
  if (missing.length) {
    reasons.push(`${missing.length}/${tokens.length} have no committed icon`);
  }
  if (conflicts.length) {
    reasons.push(
      `${conflicts.length}/${tokens.length} have more than one committed icon`,
    );
  }
  console.error(`\n--strict: ${reasons.join("; ")}.`);
  // Set the code rather than calling process.exit(): under CI both streams
  // are pipes and an explicit exit can truncate the reasons above.
  process.exitCode = 1;
}
