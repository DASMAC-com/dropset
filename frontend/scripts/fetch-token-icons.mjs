// Mirror stablecoin icons into public/token-icons at build time, so the
// browser hits our origin once instead of ~13 third-party CDNs per page load.
// Writes lib/data/icon-manifest.gen.json (symbol → /token-icons/<file>)
// which currencies.ts overlays onto the canonical remote URLs in
// currencies.json.
//
// Pass --strict (CI does, for the icon-liveness job) to also exit non-zero
// when any symbol is missing from the manifest. Without it this script
// cannot fail, so a dead issuer URL is invisible: the icons and the
// manifest are both gitignored, leaving no committed baseline to diff.
//
// The fetching itself — timeout, retry, body-size floor, magic-byte format
// detection — lives in mirror-icons.mjs, shared with the wallet script.
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { mirrorIcons } from "./mirror-icons.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const data = JSON.parse(
  readFileSync(resolve(here, "../lib/data/currencies.json"), "utf8"),
);

const tokens = Object.values(data).flatMap((entry) => entry.stablecoins);

await mirrorIcons({
  // The mint rides along in the failure label so a maintainer chasing a
  // dead URL can identify the token without a second lookup.
  items: tokens.map((s) => ({
    key: s.symbol,
    url: s.icon,
    label: `${s.symbol} (${s.mint})`,
  })),
  dst: resolve(here, "../public/token-icons"),
  manifestPath: resolve(here, "../lib/data/icon-manifest.gen.json"),
  urlPrefix: "/token-icons",
  noun: "token",
  strict: process.argv.includes("--strict"),
});
