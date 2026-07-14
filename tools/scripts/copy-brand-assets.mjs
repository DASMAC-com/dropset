// Copy the repo's shared brand assets into an app's public/ at dev/build
// time. The single real copy of each asset lives in the repo-root
// brand-assets/ folder; each app (frontend, decks) sources from it here
// rather than committing a duplicate (or a symlink that escapes the app's
// Vercel Root Directory and may not survive Vercel's build-time static
// collection).
//
// Usage: node tools/scripts/copy-brand-assets.mjs <dest-dir>
//   where <dest-dir> is the app's public/ dir relative to the repo root,
//   e.g. `frontend/public` or `decks/public`.
import { copyFileSync, mkdirSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..");
const source = join(repoRoot, "brand-assets");

const destArg = process.argv[2];
if (!destArg) {
  console.error("usage: node tools/scripts/copy-brand-assets.mjs <dest-dir>");
  process.exit(1);
}
// Resolve the destination against the repo root so the argv is independent
// of the caller's cwd (Vercel runs each app's build from its Root Directory).
const dest = resolve(repoRoot, destArg);

// Copy the contents of brand-assets/ rather than a hardcoded list, so a new
// shared brand asset is a drop-in file with no edit to this script.
const assets = readdirSync(source, { withFileTypes: true })
  .filter((entry) => entry.isFile())
  .map((entry) => entry.name);

mkdirSync(dest, { recursive: true });
for (const asset of assets) {
  copyFileSync(join(source, asset), join(dest, asset));
}

console.log(`Copied ${assets.length} brand asset(s) into ${destArg}.`);
