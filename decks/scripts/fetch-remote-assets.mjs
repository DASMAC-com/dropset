// Mirror the decks' remote images into public/remote/ at dev/build time, from
// the `<filename>: <url>` manifest in remote-assets.json. These are images we
// don't own a copy of — the team headshots served by the marketing site, and
// third-party logos — so they're sourced from their canonical URL rather than
// committed, and the mirrored files are gitignored.
//
// Unlike frontend's icon mirrors, which degrade to a remote URL when a fetch
// fails, this script **hard-fails**: any asset that can't be mirrored exits
// non-zero, which fails the `predev` / `prebuild` hook and so fails the whole
// deck build — locally, on Vercel, and in CI. A deck that can't show a face or
// a logo should not build at all, rather than build with a broken image nobody
// notices until it's on the projector.
//
// Usage: node scripts/fetch-remote-assets.mjs
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const manifestPath = resolve(here, "../remote-assets.json");
const dest = resolve(here, "../public/remote");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));

// A CDN blip shouldn't fail a build, but a wrong URL must. Retry a few times
// with a widening delay, then give up and let the failure stand.
const ATTEMPTS = 3;
const TIMEOUT_MS = 15_000;
const RETRY_DELAY_MS = 750;

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

async function download(url) {
  let lastError;
  for (let attempt = 1; attempt <= ATTEMPTS; attempt++) {
    try {
      const res = await fetch(url, {
        redirect: "follow",
        signal: AbortSignal.timeout(TIMEOUT_MS),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      // A CDN that answers a dead asset with an HTML error page still returns
      // 200, so check the type rather than trusting the status alone.
      const type = res.headers.get("content-type")?.split(";")[0]?.trim() ?? "";
      if (!type.startsWith("image/")) {
        throw new Error(`not an image (content-type: ${type || "none"})`);
      }
      const body = Buffer.from(await res.arrayBuffer());
      if (body.length === 0) throw new Error("empty response body");
      return body;
    } catch (error) {
      lastError = error;
      if (attempt < ATTEMPTS) await sleep(RETRY_DELAY_MS * attempt);
    }
  }
  throw lastError;
}

// Clear the directory first so a filename dropped from the manifest doesn't
// linger and keep a slide that references it working by accident.
rmSync(dest, { recursive: true, force: true });
mkdirSync(dest, { recursive: true });

const names = Object.keys(manifest);
const results = await Promise.allSettled(
  names.map(async (name) => {
    writeFileSync(resolve(dest, name), await download(manifest[name]));
  }),
);

const failures = results
  .map((result, i) => ({ name: names[i], result }))
  .filter(({ result }) => result.status === "rejected");

if (failures.length > 0) {
  console.error(
    `Failed to mirror ${failures.length}/${names.length} remote asset(s):`,
  );
  for (const { name, result } of failures) {
    console.error(`  - ${name} (${manifest[name]}): ${result.reason.message}`);
  }
  console.error(
    "Fix the URL in decks/remote-assets.json, or re-run once the host is back.",
  );
  process.exit(1);
}

console.log(`Mirrored ${names.length} remote asset(s) into decks/public/remote.`);
