// Mirror stablecoin icons into public/token-icons at build time, so the
// browser hits our origin once instead of ~13 third-party CDNs per page load.
// Writes lib/data/icon-manifest.gen.json (symbol → /token-icons/<file>)
// which currencies.ts overlays onto the canonical remote URLs in
// currencies.json. Manifest is always written (even if empty / all
// fetches failed) so the static TS import in currencies.ts never breaks.
//
// Pass --strict (CI does, for the icon-liveness job) to also exit non-zero
// when any symbol is missing from the manifest. Without it this script
// cannot fail, so a dead issuer URL is invisible: the icons and the
// manifest are both gitignored, leaving no committed baseline to diff.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const data = JSON.parse(
  readFileSync(resolve(here, "../lib/data/currencies.json"), "utf8"),
);
const dst = resolve(here, "../public/token-icons");
const manifestPath = resolve(here, "../lib/data/icon-manifest.gen.json");

const EXT_BY_CT = {
  "image/png": "png",
  "image/svg+xml": "svg",
  "image/webp": "webp",
  "image/jpeg": "jpg",
  "image/gif": "gif",
};

rmSync(dst, { recursive: true, force: true });
mkdirSync(dst, { recursive: true });

const STRICT = process.argv.includes("--strict");

const manifest = {};
const failures = [];

// Smallest plausible icon. Guards against a CDN answering 200 with an
// empty or near-empty body, which would otherwise mirror as a file that
// looks valid on disk but that no browser can render.
const MIN_BYTES = 64;
const ATTEMPTS = 3;
const BACKOFF_MS = 500;
const TIMEOUT_MS = 10_000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const fetchOnce = async (url) => {
  const res = await fetch(url, {
    redirect: "follow",
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  // Only mirror content-types we recognize as images. Deriving the
  // extension from an arbitrary content-type instead let an HTTP 200
  // carrying an HTML error page or CDN interstitial through `res.ok`
  // above, write `SYMBOL.html`, and manifest it as a *success* — a
  // broken icon indistinguishable from a working one. Rejecting here
  // drops the symbol from the manifest, so currencies.ts leaves its
  // canonical remote URL in place.
  const ct = res.headers.get("content-type")?.split(";")[0]?.trim() ?? "";
  const ext = EXT_BY_CT[ct];
  if (!ext) throw new Error(`unexpected content-type ${ct || "(none)"}`);
  const buf = Buffer.from(await res.arrayBuffer());
  if (buf.length < MIN_BYTES) throw new Error(`body is ${buf.length}B`);
  // The header can also be right while the body is not — sniff SVG rather
  // than trusting `image/svg+xml` alone, since that is the one type an
  // HTML interstitial can plausibly be served as. Scan the whole body, not
  // a fixed prefix: an issuer logo can carry a long XML declaration,
  // DOCTYPE, and license banner ahead of the root element, and a false
  // reject here now blocks merges.
  if (ext === "svg" && !buf.toString("utf8").includes("<svg")) {
    throw new Error("content-type is SVG but body has no <svg> tag");
  }
  return { ext, buf };
};

// Retry before declaring a URL dead: under --strict a single transient
// blip would otherwise fail the build and read as link rot. Only a URL
// that fails every attempt is treated as broken.
const fetchWithRetry = async (url) => {
  let lastError;
  for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
    if (attempt > 0) await sleep(BACKOFF_MS * 2 ** (attempt - 1));
    try {
      return await fetchOnce(url);
    } catch (err) {
      lastError = err;
    }
  }
  throw lastError;
};

const tokens = Object.values(data).flatMap((entry) => entry.stablecoins);
const results = await Promise.allSettled(
  tokens.map(async (s) => {
    const { ext, buf } = await fetchWithRetry(s.icon);
    const filename = `${s.symbol}.${ext}`;
    writeFileSync(resolve(dst, filename), buf);
    return { symbol: s.symbol, filename };
  }),
);

for (let i = 0; i < results.length; i++) {
  const r = results[i];
  const s = tokens[i];
  if (r.status === "fulfilled") {
    manifest[r.value.symbol] = `/token-icons/${r.value.filename}`;
  } else {
    failures.push(`${s.symbol} (${s.mint}): ${r.reason.message}`);
  }
}

writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

console.log(
  `Mirrored ${Object.keys(manifest).length}/${tokens.length} token icons → public/token-icons`,
);
if (failures.length) {
  console.warn(`  ${failures.length} failed (will fall back to remote URLs):`);
  for (const f of failures) console.warn(`  - ${f}`);
}

// The manifest is written either way, so a strict failure still leaves a
// buildable tree; the non-zero exit is what makes a dead issuer URL
// visible. Off by default because a normal `pnpm dev` should degrade to
// the remote URLs rather than refuse to start.
if (STRICT && failures.length) {
  console.error(
    `\n--strict: ${failures.length}/${tokens.length} token icon(s) failed after ${ATTEMPTS} attempts (see reasons above).`,
  );
  // Set the code rather than calling process.exit(): under CI both streams
  // are pipes, and an explicit exit can truncate the reasons printed above
  // before they flush. Nothing is pending after this, so the process ends
  // here either way.
  process.exitCode = 1;
}
