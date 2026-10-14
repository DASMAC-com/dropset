/**
 * Pin the TS copy of each program id to its committed source. The Rust
 * mirror is `sdk/rs/tests/program_id.rs`, which also pins the program's
 * `declare_id!`s.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import {
  DROPSET_MAINNET_PROGRAM_ADDRESS,
  dropsetProgramAddress,
} from './cluster';
import { DROPSET_PROGRAM_ADDRESS } from './generated';

test('the mainnet id matches the committed public key file', () => {
  const file = readFileSync(
    new URL('../../../keys/dropset-mainnet.pubkey', import.meta.url),
    'utf8',
  );
  assert.strictEqual(DROPSET_MAINNET_PROGRAM_ADDRESS, file.trim());
});

test('each cluster resolves its own program id', () => {
  assert.strictEqual(dropsetProgramAddress('localnet'), DROPSET_PROGRAM_ADDRESS);
  assert.strictEqual(
    dropsetProgramAddress('mainnet'),
    DROPSET_MAINNET_PROGRAM_ADDRESS,
  );
  assert.notStrictEqual(DROPSET_PROGRAM_ADDRESS, DROPSET_MAINNET_PROGRAM_ADDRESS);
});
