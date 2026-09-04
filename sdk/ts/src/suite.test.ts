/**
 * Guard the test runner's own file discovery.
 *
 * `package.json`'s test script globs `src/*.test.ts`, expanded by the
 * package-script shell rather than by node — node only gained `--test` glob
 * support in v21, this repo declares no `engines` floor, and under POSIX
 * `sh` a doubled star degrades to a single one — so the recursive spelling
 * would expand to a one-level pattern that matches only inside a
 * subdirectory, dropping every flat test file. The flat glob is therefore
 * the correct form, and its one cost is that a test file placed in a
 * subdirectory would be silently skipped.
 *
 * That is exactly the failure the glob was introduced to end: the script
 * used to enumerate its files by name, and `clock.test.ts` was missing from
 * the list, so its eight cases had never run in CI. Swapping a
 * silently-omitted list for a silently-unmatched glob would leave the class
 * open, so this asserts the property the glob depends on — every test file
 * sits directly under `src/` — and fails loudly the moment someone nests
 * one.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const SRC = fileURLToPath(new URL('.', import.meta.url));
const PKG = new URL('../package.json', import.meta.url);

/** Every `*.test.ts` at or below `src/`, as paths relative to `src/`. */
function findTests(dir: string, prefix = ''): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      out.push(...findTests(`${dir}/${entry.name}`, rel));
    } else if (entry.name.endsWith('.test.ts')) {
      out.push(rel);
    }
  }
  return out;
}

// The guard below is only meaningful while the runner actually globs. Assert
// the dependency, not just the dependent property: reverting the script to an
// enumerated list would otherwise leave this file green while reopening the
// very class it exists to close — and its failure message would then name a
// glob no longer present.
test('the test script still discovers its files by glob', () => {
  const pkg = JSON.parse(readFileSync(PKG, 'utf8')) as {
    scripts: Record<string, string>;
  };
  assert.match(
    pkg.scripts.test,
    /src\/\*\.test\.ts/,
    'package.json\'s test script must glob "src/*.test.ts" — an enumerated ' +
      'list silently omits any file missing from it, which is the bug this ' +
      'guard and that glob were introduced to fix',
  );
});

test('every test file sits directly under src/, so the flat glob finds it', () => {
  const nested = findTests(SRC).filter((p) => p.includes('/'));
  assert.deepEqual(
    nested,
    [],
    `these test files are below src/ and so are NOT run by the "src/*.test.ts" ` +
      `glob in package.json — move them up, or switch the script to node's own ` +
      `recursive glob once both CI and local development are on node >= 21`,
  );
});
