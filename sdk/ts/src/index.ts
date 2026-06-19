/**
 * `@dropset/sdk` — TypeScript client for the Dropset eCLOB program.
 *
 * Re-exports the Codama-generated `@solana/kit` client (instruction
 * builders, account & event codecs, PDA helpers, program constants) and
 * the hand-written {@link ./price | Price codec}. Regenerate the
 * `generated/` tree with `make sdk` after `make idl`.
 */

export * from './generated';
export * from './price';
export * from './quoting';
export * from './share';
