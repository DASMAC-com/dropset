// Copy the shared brand assets from the frontend's public/ into the deck's
// public/ at dev/build time. The deck and the frontend live in the same
// monorepo, so the frontend holds the single real copy of each asset; the
// deck sources from it here rather than committing a duplicate (or a symlink
// that escapes the deck's Vercel Root Directory and may not survive Vercel's
// build-time static collection).
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const deckPublic = join(here, "..", "public");
const frontendPublic = join(here, "..", "..", "frontend", "public");

// Assets the deck needs from the frontend. favicon-with-stroke.svg is the
// stroked favicon variant that clears Safari's low-contrast white-chip
// heuristic on the brand blue — see the rationale in frontend/app/layout.tsx.
const assets = ["dropset-wordmark.png", "favicon-with-stroke.svg"];

mkdirSync(deckPublic, { recursive: true });
for (const asset of assets) {
  copyFileSync(join(frontendPublic, asset), join(deckPublic, asset));
}
