// Mirror wallet brand icons into public/wallet-icons at build time, so the
// picker hits our own origin instead of third-party CDNs (and so wallets that
// aren't installed still show a real logo rather than a letter avatar).
// Writes lib/data/wallet-manifest.gen.json (key → /wallet-icons/<file>) which
// wallets.ts overlays onto the canonical remote URLs in wallets.json.
//
// Pass --strict (CI does, for the icon-liveness job) to also exit non-zero
// when any wallet is missing from the manifest, on the same reasoning as the
// token script: the icons and the manifest are both gitignored, so without a
// failing exit a dead issuer URL leaves no committed baseline to diff.
//
// The fetching itself — timeout, retry, body-size floor, magic-byte format
// detection — lives in mirror-icons.mjs, shared with the token script.
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { mirrorIcons } from "./mirror-icons.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const wallets = JSON.parse(
  readFileSync(resolve(here, "../lib/data/wallets.json"), "utf8"),
);

await mirrorIcons({
  items: wallets.map((w) => ({ key: w.key, url: w.icon })),
  dst: resolve(here, "../public/wallet-icons"),
  manifestPath: resolve(here, "../lib/data/wallet-manifest.gen.json"),
  urlPrefix: "/wallet-icons",
  noun: "wallet",
  strict: process.argv.includes("--strict"),
});
