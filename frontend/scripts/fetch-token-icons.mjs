// Mirror stablecoin icons into public/token-icons at build time, so the
// browser hits our origin once instead of ~13 third-party CDNs per page load.
// Writes lib/icon-manifest.gen.json (mint → /token-icons/<file>) which
// currencies.ts overlays onto the canonical remote URLs in currencies.json.
// Manifest is always written (even if empty / all fetches failed) so the
// static TS import in currencies.ts never breaks.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const data = JSON.parse(
  readFileSync(resolve(here, "../lib/currencies.json"), "utf8"),
);
const dst = resolve(here, "../public/token-icons");
const manifestPath = resolve(here, "../lib/icon-manifest.gen.json");

const EXT_BY_CT = {
  "image/png": "png",
  "image/svg+xml": "svg",
  "image/webp": "webp",
  "image/jpeg": "jpg",
  "image/gif": "gif",
};

rmSync(dst, { recursive: true, force: true });
mkdirSync(dst, { recursive: true });

const manifest = {};
const failures = [];

const tokens = Object.values(data).flatMap((entry) => entry.stablecoins);
const results = await Promise.allSettled(
  tokens.map(async (s) => {
    const res = await fetch(s.icon, { redirect: "follow" });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const ct = res.headers.get("content-type")?.split(";")[0]?.trim() ?? "";
    const ext = EXT_BY_CT[ct] ?? ct.split("/")[1] ?? "bin";
    const filename = `${s.mint}.${ext}`;
    const buf = Buffer.from(await res.arrayBuffer());
    writeFileSync(resolve(dst, filename), buf);
    return { mint: s.mint, filename };
  }),
);

for (let i = 0; i < results.length; i++) {
  const r = results[i];
  const s = tokens[i];
  if (r.status === "fulfilled") {
    manifest[r.value.mint] = `/token-icons/${r.value.filename}`;
  } else {
    failures.push(`${s.symbol} (${s.mint}): ${r.reason.message}`);
  }
}

writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

console.log(`Mirrored ${Object.keys(manifest).length}/${tokens.length} token icons → public/token-icons`);
if (failures.length) {
  console.warn(`  ${failures.length} failed (will fall back to remote URLs):`);
  for (const f of failures) console.warn(`  - ${f}`);
}
