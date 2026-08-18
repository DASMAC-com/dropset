/**
 * `initSimulator`'s memo semantics — specifically, that a *failed*
 * instantiation is not cached.
 *
 * In its own file on purpose. The memo lives at module scope, and this is the
 * one test that needs it to start empty and then be poisoned by a failure;
 * every other suite instantiates successfully at import time, which would
 * satisfy the memo before the failing call could be made. `node --test` runs
 * each file in a separate process, so the failure here cannot leak into them.
 *
 * What this pins: the shared WASM module backs both the swap simulator and
 * the resting-book read, and the consumers of both run self-healing retry
 * loops. Memoizing a rejection would turn a transient load failure into a
 * permanent one — the loops would spin forever against a promise that can
 * never resolve, recovering only on a page reload.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { initSimulator } from './simulate';

const WASM_PATH = new URL('./wasm/dropset_interface_bg.wasm', import.meta.url);

// Not a valid WASM module: fails the magic-number check, so instantiation
// rejects the way a truncated or intercepted fetch would.
const CORRUPT = Uint8Array.from([0, 1, 2, 3]);

test('a failed init is not memoized, so a later call retries', async () => {
  // Function form: this rejects asynchronously, but the assertion also covers
  // a synchronous throw, so the test pins the behavior either way.
  await assert.rejects(() => initSimulator(CORRUPT));

  // The retry. If the rejection had been cached this would replay it rather
  // than instantiating, which is the regression being guarded.
  await initSimulator(readFileSync(WASM_PATH));

  // And the *successful* instantiation is memoized: a third call resolves
  // against the same module instead of re-instantiating. Passing the corrupt
  // bytes here would reject if the memo had not taken.
  await initSimulator(CORRUPT);
});
