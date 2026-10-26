/**
 * Pin the event envelope this module hand-copies — the anchor `emit_cpi!` tag
 * and the `FillEvent` discriminator — and cover the emitting-program check that
 * keeps a spoofed event out of the tape.
 *
 * The Rust mirror is the `tests` module in `sdk/rs/src/events.rs` (constants)
 * plus `collect_fills`'s tests in `tui/src/fills.rs` (attribution).
 */

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import { address, getBase58Decoder, getBase58Encoder } from '@solana/kit';

import { DROPSET_MAINNET_PROGRAM_ADDRESS } from './cluster';
import {
  collectFillEvents,
  decodeFillEventPayload,
  EVENT_DISCRIMINATOR_LEN,
  EVENT_IX_TAG_LE,
  eventAccountKeys,
  FILL_EVENT_DISCRIMINATOR,
  stripEventTag,
  type EventTransaction,
} from './events';
import {
  DROPSET_PROGRAM_ADDRESS,
  getFillEventEncoder,
  getSetTakerFeeEventDecoder,
  getSetTakerFeeEventEncoder,
  type FillEventArgs,
} from './generated';

// A stand-in address for the event's own fields — their values don't matter to
// extraction, only that they round-trip.
const SOME_ADDRESS = address('11111111111111111111111111111112');

const sampleEvent = (): FillEventArgs => ({
  market: SOME_ADDRESS,
  taker: SOME_ADDRESS,
  leader: SOME_ADDRESS,
  quoteAuthority: SOME_ADDRESS,
  sectorIdx: 3,
  levelIdx: 5,
  vaultSeq: 17n,
  fillPrice: 0x0001_0000,
  flipPrice: 0,
  fillBase: 1_000_000n,
  fillQuote: 1_100_000n,
  remainingAfter: 4_000_000n,
  baseAtomsAfter: 9_000_000n,
  quoteAtomsAfter: 8_000_000n,
  nonceAfter: 42n,
  takerFeeAtoms: 300n,
  side: 1,
  legKind: 0,
  reserved: new Uint8Array(46),
});

/** Wrap an encoded event body in the `[tag][discriminator][body]` envelope. */
const envelope = (event: FillEventArgs): Uint8Array => {
  const body = getFillEventEncoder().encode(event);
  const out = new Uint8Array(
    EVENT_IX_TAG_LE.length + EVENT_DISCRIMINATOR_LEN + body.length,
  );
  out.set(EVENT_IX_TAG_LE, 0);
  out.set(FILL_EVENT_DISCRIMINATOR, EVENT_IX_TAG_LE.length);
  out.set(body, EVENT_IX_TAG_LE.length + EVENT_DISCRIMINATOR_LEN);
  return out;
};

/** Build a one-inner-instruction transaction around some event-CPI data. */
const txWith = (
  data: Uint8Array,
  {
    programIdIndex = 1,
    accountKeys = [SOME_ADDRESS as string, DROPSET_PROGRAM_ADDRESS as string],
    loadedAddresses,
  }: {
    programIdIndex?: number;
    accountKeys?: readonly string[];
    loadedAddresses?: {
      readonly: readonly string[];
      writable: readonly string[];
    };
  } = {},
): EventTransaction => ({
  meta: {
    innerInstructions: [
      {
        instructions: [
          { programIdIndex, data: getBase58Decoder().decode(data) },
        ],
      },
    ],
    loadedAddresses,
  },
  transaction: { message: { accountKeys } },
});

/**
 * Build a transaction from explicit inner-instruction sets, so a test can
 * express the multi-leg and mixed-traffic shapes `txWith` can't: a sweep emits
 * one FillEvent per level, and a real transaction's sets also carry token
 * transfers and other programs' instructions.
 */
const txWithSets = (
  sets: readonly { programIdIndex: number; data: Uint8Array }[][],
): EventTransaction => ({
  meta: {
    innerInstructions: sets.map((instructions) => ({
      instructions: instructions.map(({ programIdIndex, data }) => ({
        programIdIndex,
        data: getBase58Decoder().decode(data),
      })),
    })),
  },
  transaction: {
    message: {
      accountKeys: [SOME_ADDRESS as string, DROPSET_PROGRAM_ADDRESS as string],
    },
  },
});

test('the event tag matches the Rust EVENT_IX_TAG_LE literal', () => {
  // 0x1d9acb512ea545e4, little-endian — mirrored from sdk/rs/src/events.rs.
  const expected = new Uint8Array(8);
  let tag = 0x1d9a_cb51_2ea5_45e4n;
  for (let i = 0; i < 8; i++) {
    expected[i] = Number(tag & 0xffn);
    tag >>= 8n;
  }
  assert.deepStrictEqual(new Uint8Array(EVENT_IX_TAG_LE), expected);
});

test('the fill discriminator matches the anchor sha256 scheme', () => {
  const digest = createHash('sha256').update('event:FillEvent').digest();
  assert.deepStrictEqual(
    new Uint8Array(FILL_EVENT_DISCRIMINATOR),
    new Uint8Array(digest.subarray(0, EVENT_DISCRIMINATOR_LEN)),
  );
});

test('a fill event round-trips through the envelope', () => {
  const event = sampleEvent();
  const fills = collectFillEvents(txWith(envelope(event)), 'localnet');
  assert.equal(fills.length, 1);
  assert.equal(fills[0].fillBase, event.fillBase);
  assert.equal(fills[0].fillQuote, event.fillQuote);
  assert.equal(fills[0].fillPrice, event.fillPrice);
  assert.equal(fills[0].side, event.side);
  assert.equal(fills[0].takerFeeAtoms, event.takerFeeAtoms);
  assert.equal(fills[0].market, event.market);
});

test('a fill against a flipped counter reads as the taker direction', () => {
  // A Buy that fills `bids[2]`'s flipped `flip_size` (a filled bid's base
  // proceeds resting as an ask) emits `side` 0 with `levelIdx` naming the
  // bids entry, `legKind` 1 naming the flipped counter, and `fillPrice` and
  // `flipPrice` both that level's `flip_price`. The reader carries every
  // field verbatim; `legKind`, not `side`, names the counter filled. This
  // documents that contract at the reader: the reader has no flip-specific
  // path, and the engine emitting it is pinned in
  // programs/dropset/tests/flip.rs.
  const flipPrice = 0x0001_0020;
  const event = {
    ...sampleEvent(),
    side: 0,
    levelIdx: 2,
    fillPrice: flipPrice,
    flipPrice,
    legKind: 1,
    remainingAfter: 0n,
  };
  const [fill] = collectFillEvents(txWith(envelope(event)), 'localnet');
  assert.ok(fill);
  assert.equal(fill.side, 0);
  assert.equal(fill.levelIdx, 2);
  assert.equal(fill.legKind, 1);
  assert.equal(fill.fillPrice, flipPrice);
  assert.equal(fill.flipPrice, flipPrice);
  assert.equal(fill.remainingAfter, 0n);
});

test('collects only events our program emitted', () => {
  // Same well-formed event bytes, but attributed to a different program: the
  // tag and discriminator are public, so this must not be trusted.
  const fills = collectFillEvents(
    txWith(envelope(sampleEvent()), { programIdIndex: 0 }),
    'localnet',
  );
  assert.deepStrictEqual(fills, []);
});

test('collects against the program id of the cluster it is given', () => {
  // A mainnet emission: trusted on mainnet, not on localnet, and the reverse
  // for a localnet one.
  const mainnetTx = txWith(envelope(sampleEvent()), {
    accountKeys: [SOME_ADDRESS, DROPSET_MAINNET_PROGRAM_ADDRESS],
  });
  assert.strictEqual(collectFillEvents(mainnetTx, 'mainnet').length, 1);
  assert.deepStrictEqual(collectFillEvents(mainnetTx, 'localnet'), []);
  const localnetTx = txWith(envelope(sampleEvent()));
  assert.deepStrictEqual(collectFillEvents(localnetTx, 'mainnet'), []);
});

test('drops events with an out-of-range program index', () => {
  const fills = collectFillEvents(
    txWith(envelope(sampleEvent()), { programIdIndex: 9 }),
    'localnet',
  );
  assert.deepStrictEqual(fills, []);
});

test('resolves a program id that lives in the loaded addresses', () => {
  // Static keys first, then loaded writable, then loaded readonly — so index 1
  // is the writable loaded address, not the readonly one.
  const fills = collectFillEvents(
    txWith(envelope(sampleEvent()), {
      programIdIndex: 1,
      accountKeys: [SOME_ADDRESS as string],
      loadedAddresses: {
        writable: [DROPSET_PROGRAM_ADDRESS as string],
        readonly: [SOME_ADDRESS as string],
      },
    }),
    'localnet',
  );
  assert.equal(fills.length, 1);
});

test('strip rejects non-event data', () => {
  // No tag at all.
  assert.equal(stripEventTag(new Uint8Array(32)), null);
  // Tag present, but the payload is shorter than a discriminator.
  const short = new Uint8Array(EVENT_IX_TAG_LE.length + 4);
  short.set(EVENT_IX_TAG_LE, 0);
  assert.equal(stripEventTag(short), null);
  // Tag present with a full discriminator: accepted here, rejected downstream.
  const exact = new Uint8Array(EVENT_IX_TAG_LE.length + EVENT_DISCRIMINATOR_LEN);
  exact.set(EVENT_IX_TAG_LE, 0);
  assert.notEqual(stripEventTag(exact), null);
});

test('a non-fill discriminator decodes to null', () => {
  const payload = new Uint8Array(EVENT_DISCRIMINATOR_LEN + 200);
  payload.set([1, 2, 3, 4, 5, 6, 7, 8], 0);
  assert.equal(decodeFillEventPayload(payload), null);
});

test('a truncated fill body decodes to null', () => {
  const payload = new Uint8Array(EVENT_DISCRIMINATOR_LEN + 16);
  payload.set(FILL_EVENT_DISCRIMINATOR, 0);
  assert.equal(decodeFillEventPayload(payload), null);
});

test('a Borsh event body missing its last field is rejected, not defaulted', () => {
  // Mirrors `a_body_missing_its_last_field_is_rejected` in
  // `sdk/rs/src/events.rs`: on the short direction the two SDKs agree, since a
  // changed Borsh body renames the event and no decoder defaults a missing
  // tail. The TS SDK has no payload decoder for this event, so the generated
  // codec is what a consumer would reach for.
  const body = getSetTakerFeeEventEncoder().encode({
    market: SOME_ADDRESS,
    takerFee: 7,
  });
  const decoder = getSetTakerFeeEventDecoder();
  assert.equal(decoder.decode(body).takerFee, 7);
  // Drop the u16 `takerFee`.
  assert.throws(() => decoder.decode(body.slice(0, body.length - 2)));
});

test('a generated Borsh codec ignores a wider body, unlike the Rust decoder', () => {
  // The wide direction is where the SDKs differ, so pin the difference rather
  // than leave it implied: the Rust
  // `a_wider_body_than_this_build_reads_is_rejected` returns `TrailingBytes`
  // for this body, while the fixed-size generated codec reads its prefix. See
  // `interface.md` §2, "Decode strictness".
  const body = getSetTakerFeeEventEncoder().encode({
    market: SOME_ADDRESS,
    takerFee: 7,
  });
  const wider = new Uint8Array(body.length + 8);
  wider.set(body, 0);
  wider.fill(0xff, body.length);
  assert.equal(getSetTakerFeeEventDecoder().decode(wider).takerFee, 7);
});

test('undecodable base58 inner data is skipped', () => {
  const tx: EventTransaction = {
    meta: {
      innerInstructions: [
        // `0` and `l` are not in the base58 alphabet.
        { instructions: [{ programIdIndex: 1, data: '0l0l' }] },
      ],
    },
    transaction: {
      message: {
        accountKeys: [SOME_ADDRESS as string, DROPSET_PROGRAM_ADDRESS as string],
      },
    },
  };
  assert.deepStrictEqual(collectFillEvents(tx, 'localnet'), []);
});

test('a transaction with no account keys attributes nothing', () => {
  const tx = txWith(envelope(sampleEvent()));
  const keyless: EventTransaction = {
    meta: tx.meta,
    transaction: { message: {} },
  };
  assert.equal(eventAccountKeys(keyless), null);
  assert.deepStrictEqual(collectFillEvents(keyless, 'localnet'), []);
});

test('a transaction with no inner instructions yields no fills', () => {
  assert.deepStrictEqual(
    collectFillEvents({
      meta: { innerInstructions: null },
      transaction: {
        message: { accountKeys: [DROPSET_PROGRAM_ADDRESS as string] },
      },
    }, 'localnet'),
    [],
  );
  assert.deepStrictEqual(
    collectFillEvents({
      meta: null,
      transaction: {
        message: { accountKeys: [DROPSET_PROGRAM_ADDRESS as string] },
      },
    }, 'localnet'),
    [],
  );
});

test('base58 round-trip of the envelope is byte-exact', () => {
  // Guards the assumption the extraction rests on: inner-instruction data is
  // base58, so encode/decode must be lossless for the 216-byte envelope.
  const data = envelope(sampleEvent());
  const text = getBase58Decoder().decode(data);
  assert.deepStrictEqual(new Uint8Array(getBase58Encoder().encode(text)), data);
});

test('a multi-leg sweep yields every leg in inner-instruction order', () => {
  // The common real shape: one swap clears several levels and emits one event
  // per leg. Order is part of the contract — consumers key rows off the leg
  // index, so a reordering would silently mislabel them.
  const legs = [1_000_000n, 250_000n, 1n].map((fillBase) =>
    envelope({ ...sampleEvent(), fillBase }),
  );
  const fills = collectFillEvents(
    txWithSets([legs.map((data) => ({ programIdIndex: 1, data }))]),
    'localnet',
  );
  assert.deepStrictEqual(
    fills.map((f) => f.fillBase),
    [1_000_000n, 250_000n, 1n],
  );
});

test('events are collected across sets and around unrelated instructions', () => {
  // A real transaction's inner instructions also carry token transfers and
  // other programs' calls, spread over more than one set.
  const ours = envelope({ ...sampleEvent(), fillBase: 7n });
  const alsoOurs = envelope({ ...sampleEvent(), fillBase: 9n });
  const foreign = envelope({ ...sampleEvent(), fillBase: 999n });
  const fills = collectFillEvents(
    txWithSets([
      [
        // Not an event at all: a token-transfer-shaped instruction from
        // another program.
        { programIdIndex: 0, data: new Uint8Array([1, 2, 3, 4]) },
        { programIdIndex: 1, data: ours },
      ],
      [
        // Well-formed event bytes, but emitted by someone else — the tag and
        // discriminator are public, so only the program id rejects it.
        { programIdIndex: 0, data: foreign },
        { programIdIndex: 1, data: alsoOurs },
      ],
    ]),
    'localnet',
  );
  assert.deepStrictEqual(
    fills.map((f) => f.fillBase),
    [7n, 9n],
  );
});

test('trailing bytes past the fixed-size fill body are tolerated', () => {
  // The decoder documents this bytemuck-`FillEvent`-only allowance, so pin it
  // rather than leaving the branch unexercised. The Rust decoder is strict
  // here and returns `TrailingBytes`; see `interface.md` §2, "Decode
  // strictness".
  const exact = envelope(sampleEvent());
  const padded = new Uint8Array(exact.length + 16);
  padded.set(exact, 0);
  const fills = collectFillEvents(txWith(padded), 'localnet');
  assert.equal(fills.length, 1);
  assert.equal(fills[0].fillBase, sampleEvent().fillBase);
});

test('a failed transaction yields no fills', () => {
  // A failed transaction still carries the inner instructions recorded before
  // it failed, and a foreign program can get event-shaped bytes recorded
  // against our program id that way — so the failure itself is the gate.
  const tx = txWith(envelope(sampleEvent()));
  assert.deepStrictEqual(
    collectFillEvents({
      ...tx,
      meta: { ...tx.meta, err: { InstructionError: [0, 'Custom'] } },
    }, 'localnet'),
    [],
  );
  // A successful transaction carries `err: null` — that must still decode.
  assert.equal(
    collectFillEvents({ ...tx, meta: { ...tx.meta, err: null } }, 'localnet').length,
    1,
  );
});

test('a malformed loadedAddresses fails closed instead of throwing', () => {
  // Every other bad-input path returns null/[]; a hostile or buggy RPC
  // shouldn't be able to throw out of the decoder and take the caller with it.
  const tx = txWith(envelope(sampleEvent()));
  const malformed = {
    ...tx,
    meta: {
      ...tx.meta,
      // Shape the declared type forbids, so cast past it deliberately.
      loadedAddresses: { writable: null, readonly: undefined },
    },
  } as unknown as EventTransaction;
  assert.doesNotThrow(() => collectFillEvents(malformed, 'localnet'));
  assert.equal(collectFillEvents(malformed, 'localnet').length, 1);
});
