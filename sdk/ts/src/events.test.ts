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

import {
  collectFillEvents,
  decodeFillEventPayload,
  DISCRIMINATOR_LEN,
  EVENT_IX_TAG_LE,
  eventAccountKeys,
  FILL_EVENT_DISCRIMINATOR,
  stripEventTag,
  type EventTransaction,
} from './events';
import {
  DROPSET_PROGRAM_ADDRESS,
  getFillEventEncoder,
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
  side: 1,
  pad: new Uint8Array(7),
  sectorIdx: 3,
  levelIdx: 5,
  fillBase: 1_000_000n,
  fillQuote: 1_100_000n,
  fillPrice: 0x0001_0000,
  pad2: new Uint8Array(4),
  baseAtomsAfter: 9_000_000n,
  quoteAtomsAfter: 8_000_000n,
  nonceAfter: 42n,
  takerFeeAtoms: 300n,
});

/** Wrap an encoded event body in the `[tag][discriminator][body]` envelope. */
const envelope = (event: FillEventArgs): Uint8Array => {
  const body = getFillEventEncoder().encode(event);
  const out = new Uint8Array(
    EVENT_IX_TAG_LE.length + DISCRIMINATOR_LEN + body.length,
  );
  out.set(EVENT_IX_TAG_LE, 0);
  out.set(FILL_EVENT_DISCRIMINATOR, EVENT_IX_TAG_LE.length);
  out.set(body, EVENT_IX_TAG_LE.length + DISCRIMINATOR_LEN);
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
    new Uint8Array(digest.subarray(0, DISCRIMINATOR_LEN)),
  );
});

test('a fill event round-trips through the envelope', () => {
  const event = sampleEvent();
  const fills = collectFillEvents(txWith(envelope(event)));
  assert.equal(fills.length, 1);
  assert.equal(fills[0].fillBase, event.fillBase);
  assert.equal(fills[0].fillQuote, event.fillQuote);
  assert.equal(fills[0].fillPrice, event.fillPrice);
  assert.equal(fills[0].side, event.side);
  assert.equal(fills[0].takerFeeAtoms, event.takerFeeAtoms);
  assert.equal(fills[0].market, event.market);
});

test('collects only events our program emitted', () => {
  // Same well-formed event bytes, but attributed to a different program: the
  // tag and discriminator are public, so this must not be trusted.
  const fills = collectFillEvents(
    txWith(envelope(sampleEvent()), { programIdIndex: 0 }),
  );
  assert.deepStrictEqual(fills, []);
});

test('drops events with an out-of-range program index', () => {
  const fills = collectFillEvents(
    txWith(envelope(sampleEvent()), { programIdIndex: 9 }),
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
  const exact = new Uint8Array(EVENT_IX_TAG_LE.length + DISCRIMINATOR_LEN);
  exact.set(EVENT_IX_TAG_LE, 0);
  assert.notEqual(stripEventTag(exact), null);
});

test('a non-fill discriminator decodes to null', () => {
  const payload = new Uint8Array(DISCRIMINATOR_LEN + 200);
  payload.set([1, 2, 3, 4, 5, 6, 7, 8], 0);
  assert.equal(decodeFillEventPayload(payload), null);
});

test('a truncated fill body decodes to null', () => {
  const payload = new Uint8Array(DISCRIMINATOR_LEN + 16);
  payload.set(FILL_EVENT_DISCRIMINATOR, 0);
  assert.equal(decodeFillEventPayload(payload), null);
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
  assert.deepStrictEqual(collectFillEvents(tx), []);
});

test('a transaction with no account keys attributes nothing', () => {
  const tx = txWith(envelope(sampleEvent()));
  const keyless: EventTransaction = {
    meta: tx.meta,
    transaction: { message: {} },
  };
  assert.equal(eventAccountKeys(keyless), null);
  assert.deepStrictEqual(collectFillEvents(keyless), []);
});

test('a transaction with no inner instructions yields no fills', () => {
  assert.deepStrictEqual(
    collectFillEvents({
      meta: { innerInstructions: null },
      transaction: {
        message: { accountKeys: [DROPSET_PROGRAM_ADDRESS as string] },
      },
    }),
    [],
  );
  assert.deepStrictEqual(
    collectFillEvents({
      meta: null,
      transaction: {
        message: { accountKeys: [DROPSET_PROGRAM_ADDRESS as string] },
      },
    }),
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
