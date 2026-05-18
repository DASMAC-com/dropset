// Mirror Twemoji's country-flag SVGs into public/flag-icons, renamed by ISO
// 3166-1 alpha-2. Run via pnpm predev / prebuild. We use Twemoji rather than
// flag-icons because Twemoji ships the unofficial regional designs people
// recognise from emoji (Mayotte's seahorse, Saint-Pierre's ship-in-canton,
// Réunion's regional banner) where flag-icons follows ISO and renders those
// territories as the bare French tricolor.
//
// Twemoji's source filenames are dash-joined regional-indicator code points
// in hex (e.g. PM → 1f1f5-1f1f2.svg). We compute the cca2 back out and copy
// each flag as `<cca2>.svg`.
import { cpSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, "../node_modules/@discordapp/twemoji/dist/svg");
const dst = resolve(here, "../public/flag-icons");

const RI_BASE = 0x1f1e6; // 🇦
const A_BASE = "A".charCodeAt(0);
const toLetter = (cp) => String.fromCharCode(A_BASE + (cp - RI_BASE));

rmSync(dst, { recursive: true, force: true });
mkdirSync(dst, { recursive: true });

let n = 0;
for (const file of readdirSync(src)) {
  const m = file.match(/^([0-9a-f]+)-([0-9a-f]+)\.svg$/);
  if (!m) continue;
  const cp1 = Number.parseInt(m[1], 16);
  const cp2 = Number.parseInt(m[2], 16);
  if (cp1 < RI_BASE || cp1 > 0x1f1ff) continue;
  if (cp2 < RI_BASE || cp2 > 0x1f1ff) continue;
  const cca2 = `${toLetter(cp1)}${toLetter(cp2)}`.toLowerCase();
  cpSync(join(src, file), join(dst, `${cca2}.svg`));
  n++;
}

console.log(`Copied ${n} country-flag SVGs from Twemoji.`);
