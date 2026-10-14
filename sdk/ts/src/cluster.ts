/**
 * Per-cluster program ids.
 *
 * The program is deployed at a different address per cluster: localnet keeps
 * the committed vanity key (the generated {@link DROPSET_PROGRAM_ADDRESS}),
 * and mainnet uses a key whose secret lives only in 1Password, its public
 * half committed at `keys/dropset-mainnet.pubkey`. Every generated
 * instruction builder and PDA helper takes a `config.programAddress`, so pass
 * {@link dropsetProgramAddress} there; the default is the localnet id.
 */

import { type Address, address } from '@solana/kit';
import { DROPSET_PROGRAM_ADDRESS } from './generated';

/** The mainnet program id, mirrored from `keys/dropset-mainnet.pubkey`. */
export const DROPSET_MAINNET_PROGRAM_ADDRESS = address(
  'FXv1Dsy4CL5UvvkAAcPmzHZCvaotJMi9TfYLraj99KUL',
);

/** The clusters the program is deployed on. */
export type DropsetCluster = 'localnet' | 'mainnet';

/** The program id deployed on `cluster`. */
export function dropsetProgramAddress(cluster: DropsetCluster): Address {
  return cluster === 'mainnet'
    ? DROPSET_MAINNET_PROGRAM_ADDRESS
    : DROPSET_PROGRAM_ADDRESS;
}
