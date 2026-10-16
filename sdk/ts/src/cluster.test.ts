/**
 * Pin the TS copy of each program id to its committed source, the refusal of
 * the localnet id off localnet, and that the generated client takes its
 * program address from the caller. The Rust mirror is
 * `sdk/rs/tests/program_id.rs`, which also pins the program's `declare_id!`s.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { address, createNoopSigner } from '@solana/kit';

import {
  assertDropsetProgramAddress,
  DROPSET_MAINNET_PROGRAM_ADDRESS,
  type DropsetCluster,
  dropsetProgramAddress,
} from './cluster';
import {
  DROPSET_PROGRAM_ADDRESS,
  findEventAuthorityPda,
  findRegistryPda,
  findVaultDepositorPda,
  getCloseRegistryInstruction,
  getCloseRegistryInstructionAsync,
  getDepositInstructionAsync,
} from './generated';

/** Neither cluster's id: a stand-in for a fresh deployment, or any wallet. */
const FRESH = address('11111111111111111111111111111112');

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

test('an unknown cluster is refused rather than read as localnet', () => {
  for (const cluster of ['devnet', 'Mainnet', '']) {
    assert.throws(
      () => dropsetProgramAddress(cluster as DropsetCluster),
      /Unknown Dropset cluster/,
    );
  }
});

test('the localnet id is refused off localnet, and only there', () => {
  assert.throws(
    () => assertDropsetProgramAddress('mainnet', DROPSET_PROGRAM_ADDRESS),
    /not deployed on mainnet/,
  );
  assert.strictEqual(
    assertDropsetProgramAddress('localnet', DROPSET_PROGRAM_ADDRESS),
    DROPSET_PROGRAM_ADDRESS,
  );
  assert.strictEqual(
    assertDropsetProgramAddress('mainnet', DROPSET_MAINNET_PROGRAM_ADDRESS),
    DROPSET_MAINNET_PROGRAM_ADDRESS,
  );
  // Any other id passes, so a fresh deployment is not locked out.
  assert.strictEqual(assertDropsetProgramAddress('mainnet', FRESH), FRESH);
});

test('generated PDA helpers require a program address and derive under it', async () => {
  // @ts-expect-error -- the program address has no default
  await assert.rejects(findRegistryPda());
  const [localnet] = await findRegistryPda({
    programAddress: dropsetProgramAddress('localnet'),
  });
  const [mainnet] = await findRegistryPda({
    programAddress: dropsetProgramAddress('mainnet'),
  });
  assert.notStrictEqual(localnet, mainnet);
});

test('generated builders require a program address', async () => {
  const input = { admin: createNoopSigner(FRESH), rentRecipient: FRESH };
  // @ts-expect-error -- the program address has no default
  await assert.rejects(getCloseRegistryInstructionAsync(input));
  assert.throws(() =>
    // @ts-expect-error -- the program address has no default
    getCloseRegistryInstruction({ ...input, registry: FRESH }),
  );
});

test('an async builder derives its defaulted PDAs under its own program', async () => {
  // The builder's registry used to be derived under the localnet id whatever
  // `config.programAddress` said (the event authority is pinned below).
  const programAddress = dropsetProgramAddress('mainnet');
  const ix = await getCloseRegistryInstructionAsync(
    { admin: createNoopSigner(FRESH), rentRecipient: FRESH },
    { programAddress },
  );
  const [registry] = await findRegistryPda({ programAddress });
  assert.strictEqual(ix.programAddress, programAddress);
  assert.ok(ix.accounts.some((a) => a.address === registry));
});

test('an inlined variable-seed PDA matches its finder', async () => {
  // The builders now derive PDAs inline rather than through the finders, so
  // pin that the inline seeds (market, u32 vault index, signer) encode the
  // same way the finder's do.
  const programAddress = dropsetProgramAddress('mainnet');
  // Distinct from the signer, so swapped address seeds would not cancel out.
  const market = address('11111111111111111111111111111113');
  const ix = await getDepositInstructionAsync(
    {
      signer: createNoopSigner(FRESH),
      market,
      baseMint: FRESH,
      quoteMint: FRESH,
      baseTokenProgram: FRESH,
      quoteTokenProgram: FRESH,
      signerBaseAta: FRESH,
      signerQuoteAta: FRESH,
      marketBaseTreasury: FRESH,
      marketQuoteTreasury: FRESH,
      program: programAddress,
      vaultIdx: 3,
      baseIn: 0n,
      quoteIn: 0n,
      maxBaseIn: 0n,
      maxQuoteIn: 0n,
    },
    { programAddress },
  );
  const [vaultDepositor] = await findVaultDepositorPda(
    { market, vaultIdx: 3, signer: FRESH },
    { programAddress },
  );
  assert.strictEqual(ix.accounts[2]?.address, vaultDepositor);
  // The event authority the swap path also relies on, derived the same way.
  const [eventAuthority] = await findEventAuthorityPda({ programAddress });
  assert.strictEqual(ix.accounts[14]?.address, eventAuthority);
});
