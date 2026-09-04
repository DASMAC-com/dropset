// Shared plumbing for the two halves of the token-icon pipeline: the
// no-network manifest build that every dev/build run performs, and the
// networked upstream audit that only the non-required CI job performs.
//
// The dependency runs repo-first: the committed bytes under
// brand-assets/token-icons/ ARE the source of truth, and each token's `icon`
// URL in currencies.json is the *declared official upstream source* for the
// asset — documentation of where the issuer publishes it, and the audit's
// input, never a build-time fetch target. Before this split the URL was
// fetched on the build path, which put six merge-queue dequeues across three
// PRs behind two different third-party hosts failing two different ways in
// one evening.
//
// The icons live under brand-assets/ rather than a new top-level directory
// because they are brand assets — other companies' brands, but brand assets
// all the same — and because copy-brand-assets.mjs already copies
// subdirectories whole, so this subtree reaches every app's public/ with no
// edit to that script.
import { readdirSync, readFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

// The committed source of truth. Copied to <app>/public/token-icons by
// copy-brand-assets.mjs, which is why the manifest's URL prefix below and
// this directory's basename have to stay the same string.
export const ICON_DIR = resolve(here, "../../brand-assets/token-icons");

// DERIVED from ICON_DIR, never written out as a literal, because the two have
// to agree and nothing else checks that they do: copy-brand-assets.mjs copies
// the directory into each app's public/ under its own basename, so the served
// prefix IS that basename by construction. Spelled as a literal, renaming the
// committed directory would keep every test and every gate green while serving
// 404s for all 25 icons — the manifest strings are what the tests assert on,
// and no test opens the path they name.
export const URL_PREFIX = `/${basename(ICON_DIR)}`;

// Every listed stablecoin, flattened across the currency groupings.
export const readTokens = () => {
  const data = JSON.parse(
    readFileSync(resolve(here, "../lib/data/currencies.json"), "utf8"),
  );
  return Object.values(data).flatMap((entry) => entry.stablecoins);
};

// Committed icons are named `<SYMBOL>.<ext>`, where the extension records
// the format upstream actually served. The extension is therefore not
// predictable from the symbol, which is why this reads the directory rather
// than composing a path — and why the manifest exists at all.
//
// Returns `{ filename, path }`, or undefined when nothing is committed for
// the symbol. A symbol with two committed files (a leftover `.png` beside a
// new `.svg` after an upstream format change) is a real error rather than a
// pick-one situation: both would be copied into public/ and which one the
// manifest names would depend on directory order.
export const committedIconFor = (symbol, entries) => {
  // A dotless entry has no extension to strip, so it is compared whole.
  // `slice(0, lastIndexOf("."))` would silently drop its last character
  // instead and could then match the wrong symbol.
  const stem = (name) => {
    const dot = name.lastIndexOf(".");
    return dot === -1 ? name : name.slice(0, dot);
  };
  const matches = entries.filter((name) => stem(name) === symbol);
  if (matches.length > 1) {
    throw new Error(
      `${symbol} has ${matches.length} committed icons (${matches.join(", ")}) — delete all but one`,
    );
  }
  if (matches.length === 0) return undefined;
  return { filename: matches[0], path: join(ICON_DIR, matches[0]) };
};

// Directory listing, hoisted so callers read it once rather than per symbol.
// Missing entirely is reported as empty rather than thrown: the manifest
// builder's own completeness check produces a far better message than an
// ENOENT trace, and it is the next thing every caller runs.
// Regular files only. A subdirectory whose name stems to a symbol would
// otherwise be returned as that symbol's icon, and the read of it would throw
// EISDIR from outside the audit's `allSettled` region — breaking the audit's
// exit-0-whatever-it-finds guarantee, which is one of the three layers keeping
// that job from ever blocking a merge.
export const listCommitted = () => {
  try {
    return readdirSync(ICON_DIR, { withFileTypes: true })
      .filter((entry) => entry.isFile() && !entry.name.startsWith("."))
      .map((entry) => entry.name);
  } catch (err) {
    if (err.code === "ENOENT") return [];
    throw err;
  }
};
