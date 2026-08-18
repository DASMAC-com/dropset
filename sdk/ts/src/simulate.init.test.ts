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
  const succeeded = initSimulator(readFileSync(WASM_PATH));
  await succeeded;

  // And the *successful* instantiation is memoized: a later call hands back
  // the very same promise instead of instantiating again.
  //
  // Asserted by identity rather than by passing corrupt bytes and expecting
  // them to be ignored. The wasm-bindgen shim short-circuits on its own
  // module-scope state once instantiated (`if (wasm !== undefined) return
  // wasm`), so a corrupt-input call resolves whether or not this memo took —
  // it would pass with the memo deleted entirely, and prove nothing.
  assert.equal(initSimulator(CORRUPT), succeeded);
});
