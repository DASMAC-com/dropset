// Mirror flag-icons' 4x3 SVGs into public/flag-icons so they ship as plain
// static assets. Run via pnpm predev / prebuild — Windows browsers can't
// render Unicode regional-indicator flag emoji, so the UI references these
// SVGs instead.
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, "../node_modules/flag-icons/flags/4x3");
const dst = resolve(here, "../public/flag-icons");

rmSync(dst, { recursive: true, force: true });
mkdirSync(dst, { recursive: true });
cpSync(src, dst, { recursive: true });
