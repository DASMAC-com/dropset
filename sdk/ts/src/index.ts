/**
 * `@dropset/sdk` — TypeScript client for the Dropset eCLOB program.
 *
 * Re-exports the Codama-generated `@solana/kit` client (instruction
 * builders, account & event codecs, PDA helpers, program constants) and
 * the hand-written {@link ./price | Price codec}. Regenerate the
 * `generated/` tree with `make sdk` after `make idl`.
 *
 * For swap routing, start at {@link ./router | router}: `quoteEclob` prices
 * our own book, `quoteBestRoute` prices it against an aggregator and returns
 * whichever is better.
 */

export * from './clock';
export * from './dflow';
export * from './events';
export * from './generated';
export * from './market';
export * from './price';
export * from './quoting';
export * from './route';
export * from './router';
export * from './share';
export * from './simulate';
