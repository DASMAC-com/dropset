// cspell:word ftypavif
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
  "image/avif": "avif",
  "image/x-icon": "ico",
  "image/vnd.microsoft.icon": "ico",
};

// Content-types that carry no format information. A live asset served with
// one of these is NOT link rot — `application/octet-stream` is a common S3
// and R2 default and every browser still renders the image — so sniff the
// bytes rather than failing. Without this the strict gate would block the
// merge queue over an issuer's storage config.
const GENERIC_CTS = new Set([
  "",
  "application/octet-stream",
  "binary/octet-stream",
]);

const startsWith = (buf, bytes) =>
  buf.length >= bytes.length &&
  buf.subarray(0, bytes.length).equals(Buffer.from(bytes));

// Identify a format from its magic bytes. Returns undefined for anything
// unrecognized, which keeps an HTML interstitial rejected even when it
// arrives under a generic content-type.
const sniffExt = (buf) => {
  if (startsWith(buf, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))
    return "png";
  if (startsWith(buf, [0xff, 0xd8, 0xff])) return "jpg";
  if (startsWith(buf, [0x47, 0x49, 0x46, 0x38])) return "gif";
  if (startsWith(buf, [0x00, 0x00, 0x01, 0x00])) return "ico";
  if (
    startsWith(buf, [0x52, 0x49, 0x46, 0x46]) &&
    buf.subarray(8, 12).toString() === "WEBP"
  ) {
    return "webp";
  }
  if (buf.subarray(4, 12).toString() === "ftypavif") return "avif";
  if (buf.includes("<svg")) return "svg";
  return undefined;
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
  const ct = res.headers.get("content-type")?.split(";")[0]?.trim() ?? "";
  const buf = Buffer.from(await res.arrayBuffer());
  if (buf.length < MIN_BYTES) throw new Error(`body is ${buf.length}B`);
  // Only mirror formats we actually recognize. Deriving the extension from
  // an arbitrary content-type instead let an HTTP 200 carrying an HTML
  // error page or CDN interstitial through the `res.ok` check above, write
  // `SYMBOL.html`, and manifest it as a *success* — a broken icon
  // indistinguishable from a working one. A rejected symbol is dropped from
  // the manifest, so currencies.ts leaves its canonical remote URL in place.
  //
  // A declared image type still gets its bytes checked, because the header
  // can be right while the body is not; a generic type is decided by the
  // bytes alone.
  const declared = EXT_BY_CT[ct];
  const sniffed = sniffExt(buf);
  if (!declared && !GENERIC_CTS.has(ct)) {
    throw new Error(`unexpected content-type ${ct || "(none)"}`);
  }
  if (!sniffed) {
    throw new Error(
      `body is not a recognized image (content-type ${ct || "(none)"})`,
    );
  }
  // Trust the sniffed format over a mislabeled header, so a PNG served as
  // `image/jpeg` is still mirrored under the right extension.
  return { ext: sniffed, buf };
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
