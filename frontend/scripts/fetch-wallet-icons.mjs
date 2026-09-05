// Mirror wallet brand icons into public/wallet-icons at build time, so the
// picker hits our own origin instead of third-party CDNs (and so wallets that
// aren't installed still show a real logo rather than a letter avatar).
// Writes lib/data/wallet-manifest.gen.json (key → /wallet-icons/<file>),
// which wallets.ts prefers over the canonical remote URL in wallets.json
// while keeping that URL reachable as a render-side fallback.
//
// Pass --strict (CI does, for the icon-liveness job) to also exit non-zero
// when any wallet is missing from the manifest: the icons and the manifest are
// both gitignored, so without a failing exit a dead issuer URL leaves no
// committed baseline to diff.
//
// Wallet icons are the LAST asset set still fetched on the build path. Token
// icons used to work this way too and no longer do — they are committed under
// brand-assets/token-icons/ and audited against upstream out of band, because
// a strict fetch of ~25 issuer URLs inside the required Frontend job kept
// dequeuing the merge queue. The same argument applies here in principle; it
// is five URLs from a more stable set, and moving them was deliberately left
// out of that change's scope.
//
// The fetching itself — timeout, retry, body-size floor, magic-byte format
// detection — lives in mirror-icons.mjs, whose fetch half is now shared with
// the token-icon audit rather than with a token mirror.
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
