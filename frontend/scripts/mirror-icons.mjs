// cspell:word ftypavif
// Shared engine behind fetch-token-icons.mjs and fetch-wallet-icons.mjs.
// Both mirror third-party images into public/ at build time so the browser
// hits our own origin instead of an issuer CDN, and both write a manifest
// (key → /<prefix>/<file>) that the corresponding data module reads alongside
// the canonical remote URLs. Both consume it the same way: as a lookup that
// leaves the canonical URL reachable, so a mirrored file that is missing or
// unreadable at runtime still has somewhere to fall back to.
//
// The two scripts were byte-identical in shape but not in rigor: the token
// one grew timeouts, retries, a body-size floor and magic-byte sniffing
// while the wallet one kept trusting `res.ok` plus the content-type header.
// They live here together so that gap cannot reopen — a hardening added for
// one asset set now applies to both by construction.
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

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

// Smallest plausible icon. Guards against a CDN answering 200 with an
// empty or near-empty body, which would otherwise mirror as a file that
// looks valid on disk but that no browser can render.
const MIN_BYTES = 64;
const ATTEMPTS = 3;
const BACKOFF_MS = 500;
const TIMEOUT_MS = 10_000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const startsWith = (buf, bytes) =>
  buf.length >= bytes.length &&
  buf.subarray(0, bytes.length).equals(Buffer.from(bytes));

// An SVG is XML, so a real one OPENS with `<svg` — at most behind a BOM, an
// XML declaration, comments, or a DOCTYPE. Testing for `<svg` anywhere in the
// body instead (the obvious spelling, and what this used to do) accepts any
// HTML error page that happens to embed an inline logo — which is exactly the
// interstitial this whole function exists to reject, and CDN challenge pages
// routinely carry one. Only the head is scanned: a prolog longer than this is
// not something an issuer serves as its icon.
//
// The DOCTYPE branch has to tolerate an internal subset — Illustrator emits
// `<!DOCTYPE svg PUBLIC "…" "…" [<!ENTITY …>]>` — because a false REJECT here
// is not cosmetic: under the --strict gate it fails the merge queue on a
// perfectly good icon.
const SVG_HEAD_BYTES = 1024;
const SVG_OPENS_DOCUMENT =
  /^﻿?\s*(?:<\?xml[^>]*\?>\s*|<!--[\s\S]*?-->\s*|<!DOCTYPE\s+svg[^>[]*(?:\[[\s\S]*?\]\s*)?>\s*)*<svg[\s/>]/i;

// Identify a format from its magic bytes. Returns undefined for anything
// unrecognized, which keeps an HTML interstitial rejected even when it
// arrives under a generic content-type.
//
// Exported for the unit test: the single assertion that pins this module's
// reason to exist is that an HTML page sniffs as undefined rather than as an
// image.
export const sniffExt = (buf) => {
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
  if (SVG_OPENS_DOCUMENT.test(buf.subarray(0, SVG_HEAD_BYTES).toString("utf8")))
    return "svg";
  return undefined;
};

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
  // `KEY.html`, and manifest it as a *success* — a broken icon
  // indistinguishable from a working one. A rejected entry is dropped from
  // the manifest, so the data module leaves its canonical remote URL in
  // place.
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

// Mirror a set of remote icons into a public directory and write the manifest
// the corresponding data module imports.
//
// `items` are `{ key, url, label }` — `key` is the manifest key and the
// mirrored basename, `label` is what a failure is reported under (the token
// script adds the mint, so a maintainer can identify the entry without a
// second lookup). `key` is interpolated into the written path, which is safe
// because both callers source it from committed JSON; a key from an untrusted
// source would need sanitizing first.
//
// Sets `process.exitCode` rather than returning a status: both callers are
// top-level scripts whose only job is this call.
export const mirrorIcons = async ({
  items,
  dst,
  manifestPath,
  urlPrefix,
  noun,
  strict,
}) => {
  rmSync(dst, { recursive: true, force: true });
  mkdirSync(dst, { recursive: true });

  const manifest = {};
  const failures = [];

  const results = await Promise.allSettled(
    items.map(async (item) => {
      const { ext, buf } = await fetchWithRetry(item.url);
      const filename = `${item.key}.${ext}`;
      writeFileSync(resolve(dst, filename), buf);
      return { key: item.key, filename };
    }),
  );

  for (let i = 0; i < results.length; i++) {
    const r = results[i];
    const item = items[i];
    if (r.status === "fulfilled") {
      manifest[r.value.key] = `${urlPrefix}/${r.value.filename}`;
    } else {
      failures.push(`${item.label ?? item.key}: ${r.reason.message}`);
    }
  }

  // Always written, even when empty or when every fetch failed, so the
  // static TS import in the data module never breaks.
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

  console.log(
    `Mirrored ${Object.keys(manifest).length}/${items.length} ${noun} icons → public${urlPrefix}`,
  );
  if (failures.length) {
    console.warn(
      `  ${failures.length} failed (will fall back to remote URLs):`,
    );
    for (const f of failures) console.warn(`  - ${f}`);
  }

  // The manifest is written either way, so a strict failure still leaves a
  // buildable tree; the non-zero exit is what makes a dead issuer URL
  // visible. Off by default because a normal `pnpm dev` should degrade to
  // the remote URLs rather than refuse to start.
  if (strict && failures.length) {
    console.error(
      `\n--strict: ${failures.length}/${items.length} ${noun} icon(s) failed after ${ATTEMPTS} attempts (see reasons above).`,
    );
    // Set the code rather than calling process.exit(): under CI both streams
    // are pipes, and an explicit exit can truncate the reasons printed above
    // before they flush. Nothing is pending after this, so the process ends
    // here either way.
    process.exitCode = 1;
  }
};
