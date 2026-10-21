/**
 * Per-cluster program ids.
 *
 * The program is deployed at a different address per cluster: localnet keeps
 * the committed vanity key (the generated {@link DROPSET_PROGRAM_ADDRESS}),
 * and mainnet uses a key whose secret lives only in 1Password, its public
 * half committed at `keys/dropset-mainnet.pubkey`. Nothing in the SDK
 * defaults to either: the hand-written entry points take a
 * {@link DropsetCluster}, and every generated instruction builder and PDA
 * helper requires a `config.programAddress`, which
 * {@link dropsetProgramAddress} supplies.
 */

import { type Address, address } from '@solana/kit';
import { DROPSET_PROGRAM_ADDRESS } from './generated';

/** The mainnet program id, mirrored from `keys/dropset-mainnet.pubkey`. */
export const DROPSET_MAINNET_PROGRAM_ADDRESS = address(
  'eBookZxYPxRsgi48cYW6wCaaae3XKe2ezsSfA8aVoLT',
);

/** The clusters the program is deployed on. */
export type DropsetCluster = 'localnet' | 'mainnet';

/**
 * The program id deployed on `cluster`.
 *
 * Throws on any other value rather than falling back to localnet. The
 * localnet keypair is committed, so anyone can deploy at that id on another
 * cluster. An untyped caller, or one with the wrong case, must not end up
 * trusting it, for instance as the emitter `collectFillEvents`
 * authenticates.
 */
export function dropsetProgramAddress(cluster: DropsetCluster): Address {
  switch (cluster) {
    case 'localnet':
      return DROPSET_PROGRAM_ADDRESS;
    case 'mainnet':
      return DROPSET_MAINNET_PROGRAM_ADDRESS;
    default:
      throw new Error(`Unknown Dropset cluster: ${String(cluster)}`);
  }
}

/**
 * Vet a program id taken from configuration rather than from
 * {@link dropsetProgramAddress}, returning it unchanged.
 *
 * Throws only when `cluster` is not localnet and `programAddress` is the
 * localnet vanity id, the one mistake a stale or copied config makes
 * silently. Any other id passes, so a fresh deployment still works. Mirrors
 * the Rust SDK's `Deployment::check_program_id`.
 */
export function assertDropsetProgramAddress(
  cluster: DropsetCluster,
  programAddress: Address,
): Address {
  if (cluster !== 'localnet' && programAddress === DROPSET_PROGRAM_ADDRESS) {
    throw new Error(
      `The localnet program id ${DROPSET_PROGRAM_ADDRESS} is not deployed on ${cluster}`,
    );
  }
  return programAddress;
}
