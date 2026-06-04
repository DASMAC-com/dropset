// Mirror wallet brand icons into public/wallet-icons at build time, so the
// picker hits our own origin instead of third-party CDNs (and so wallets that
// aren't installed still show a real logo rather than a letter avatar).
// Writes lib/data/wallet-manifest.gen.json (key → /wallet-icons/<file>) which
// wallets.ts overlays onto the canonical remote URLs in wallets.json. Manifest
// is always written (even if empty / all fetches failed) so the static TS
// import in wallets.ts never breaks — unmirrored icons fall back to remote.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const wallets = JSON.parse(
  readFileSync(resolve(here, "../lib/data/wallets.json"), "utf8"),
);
const dst = resolve(here, "../public/wallet-icons");
const manifestPath = resolve(here, "../lib/data/wallet-manifest.gen.json");

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

const results = await Promise.allSettled(
  wallets.map(async (w) => {
    const res = await fetch(w.icon, { redirect: "follow" });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const ct = res.headers.get("content-type")?.split(";")[0]?.trim() ?? "";
    const ext = EXT_BY_CT[ct] ?? ct.split("/")[1] ?? "bin";
    const filename = `${w.key}.${ext}`;
    const buf = Buffer.from(await res.arrayBuffer());
    writeFileSync(resolve(dst, filename), buf);
    return { key: w.key, filename };
  }),
);

for (let i = 0; i < results.length; i++) {
  const r = results[i];
  const w = wallets[i];
  if (r.status === "fulfilled") {
    manifest[r.value.key] = `/wallet-icons/${r.value.filename}`;
  } else {
    failures.push(`${w.key}: ${r.reason.message}`);
  }
}

writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

console.log(
  `Mirrored ${Object.keys(manifest).length}/${wallets.length} wallet icons → public/wallet-icons`,
);
if (failures.length) {
  console.warn(`  ${failures.length} failed (will fall back to remote URLs):`);
  for (const f of failures) console.warn(`  - ${f}`);
}
