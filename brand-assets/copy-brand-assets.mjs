// Copy the repo's brand assets into an app's public/ at dev/build time. The
// single real copy of every brand asset lives in the repo-root brand-assets/
// folder — including ones only one app renders today, so "where does this
// asset live?" has exactly one answer and gaining a second consumer needs no
// file move. Each app (frontend, decks) sources from it here rather than
// committing a duplicate (or a symlink that escapes the app's Vercel Root
// Directory and may not survive Vercel's build-time static collection).
//
// The whole folder goes to every app: the set is tens of KB total, so
// shipping the frontend's share image to the deck costs nothing next to
// splitting the source of truth per consumer.
//
// Usage: node brand-assets/copy-brand-assets.mjs <dest-dir>
//   where <dest-dir> is the app's public/ dir relative to the repo root,
//   e.g. `frontend/public` or `decks/public`.
import { copyFileSync, cpSync, mkdirSync, readdirSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..");
const source = here;
const selfName = basename(fileURLToPath(import.meta.url));

const destArg = process.argv[2];
if (!destArg) {
  console.error("usage: node brand-assets/copy-brand-assets.mjs <dest-dir>");
  process.exit(1);
}
// Resolve the destination against the repo root so the argv is independent
// of the caller's cwd (Vercel runs each app's build from its Root Directory).
const dest = resolve(repoRoot, destArg);

// Copy the contents of brand-assets/ rather than a hardcoded list, so a new
// brand asset is a drop-in file with no edit to this script. Subdirectories
// are copied whole, so assets can later be grouped (e.g. brand-assets/logos/)
// without touching this script. Skip this script's own file — it lives among
// the assets it copies, and it isn't one.
const entries = readdirSync(source, { withFileTypes: true }).filter(
  (entry) => entry.name !== selfName,
);

mkdirSync(dest, { recursive: true });
for (const entry of entries) {
  const from = join(source, entry.name);
  const to = join(dest, entry.name);
  if (entry.isDirectory()) {
    cpSync(from, to, { recursive: true });
  } else {
    copyFileSync(from, to);
  }
}

console.log(`Copied ${entries.length} brand asset(s) into ${destArg}.`);
