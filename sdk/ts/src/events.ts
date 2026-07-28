/**
 * Off-chain extraction for the program's `emit_cpi!` events — the TS
 * counterpart to `sdk/rs/src/events.rs`.
 *
 * anchor v2's `emit_cpi!` records each event as a self-CPI to the program
 * (authority = the `__event_authority` PDA), so the event lands in the
 * transaction's *inner* instructions rather than the logs. Each such
 * inner-instruction `data` is
 *
 * ```text
 * EVENT_IX_TAG_LE (8)  ++  DISCRIMINATOR (8)  ++  body
 * ```
 *
 * Codama generates only the post-extraction codec ({@link getFillEventDecoder},
 * whose explicit `pad` / `pad2` fields make the decode byte-identical to the
 * on-chain `repr(C)` bytes) — this module supplies the extraction: walk the
 * inner instructions, verify the emitting program, strip the envelope, decode.
 *
 * Scope: fills only. The Rust decoder dispatches all twelve events because the
 * indexer and the teardown path consume them; the frontend's recent-fills tape
 * needs just {@link FillEvent}, so adding a variant here is a deliberate step,
 * not an oversight.
 */

import { getBase58Encoder, type ReadonlyUint8Array } from '@solana/kit';
import {
  DROPSET_PROGRAM_ADDRESS,
  getFillEventDecoder,
  type FillEvent,
} from './generated';

/**
 * The anchor v2 `emit_cpi!` self-CPI tag, little-endian — the 8-byte prefix on
 * every event inner-instruction's data (`0x1d9acb512ea545e4`).
 *
 * Mirrors `EVENT_IX_TAG_LE` in `sdk/rs/src/events.rs`, which the program test
 * crate's `sdk_event_tag_matches_anchor` pins to the on-chain constant. The
 * test below pins this array to that Rust literal, so a fork bump that moved
 * the tag fails there rather than silently zeroing event decoding here.
 */
export const EVENT_IX_TAG_LE: ReadonlyUint8Array = new Uint8Array([
  0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d,
]);

/** Length of the discriminator that follows the tag. */
export const EVENT_DISCRIMINATOR_LEN = 8;

/**
 * `sha256("event:FillEvent")[..8]` — the anchor discriminator scheme. Kept as
 * a constant for the same reason the Rust side does it (a decoder shouldn't
 * hash at runtime); the test pins it to a real sha256.
 */
export const FILL_EVENT_DISCRIMINATOR: ReadonlyUint8Array = new Uint8Array([
  13, 89, 41, 228, 105, 178, 45, 112,
]);

/**
 * One compiled inner instruction, as `getTransaction` returns it under
 * `encoding: 'json'`. Structural rather than kit's deeply-generic
 * `TransactionInstruction` so this helper doesn't pin callers to one exact
 * `getTransaction` config's return type.
 *
 * `data` is **base58** even when the transaction itself is fetched as base64 —
 * the same quirk `tui/src/fills.rs` documents.
 */
export type EventInnerInstruction = {
  programIdIndex: number;
  data: string;
};

/** One inner-instruction set (the inner instructions of one outer index). */
export type EventInnerInstructionSet = {
  instructions: readonly EventInnerInstruction[];
};

/**
 * The slice of a `getTransaction` response this module reads. Every field is
 * optional-or-null exactly where the RPC may omit it.
 */
export type EventTransaction = {
  meta?: {
    /**
     * Non-null when the transaction failed. Load-bearing rather than
     * informational — see {@link collectFillEvents}, which refuses to read
     * events out of a failed transaction.
     */
    err?: unknown;
    innerInstructions?: readonly EventInnerInstructionSet[] | null;
    loadedAddresses?: {
      readonly: readonly string[];
      writable: readonly string[];
    } | null;
  } | null;
  transaction: {
    message: {
      accountKeys?: readonly string[];
    };
  };
};

/**
 * Strip the {@link EVENT_IX_TAG_LE} prefix from one inner-instruction's data,
 * yielding the `[discriminator][body]` payload — or `null` if this inner
 * instruction is not a Dropset event-CPI.
 */
export function stripEventTag(
  data: ReadonlyUint8Array,
): ReadonlyUint8Array | null {
  if (data.length < EVENT_IX_TAG_LE.length + EVENT_DISCRIMINATOR_LEN) return null;
  for (let i = 0; i < EVENT_IX_TAG_LE.length; i++) {
    if (data[i] !== EVENT_IX_TAG_LE[i]) return null;
  }
  return data.slice(EVENT_IX_TAG_LE.length);
}

/**
 * Decode one tag-stripped payload (`[discriminator(8)][body]`) as a
 * {@link FillEvent}, or `null` if the discriminator names a different event or
 * the body is too short.
 *
 * Trailing bytes past the fixed-size body are tolerated (borsh's `deserialize`
 * does the same on the Rust side), so only the leading `fixedSize` bytes are
 * handed to the codec.
 */
export function decodeFillEventPayload(
  payload: ReadonlyUint8Array,
): FillEvent | null {
  if (payload.length < EVENT_DISCRIMINATOR_LEN) return null;
  for (let i = 0; i < EVENT_DISCRIMINATOR_LEN; i++) {
    if (payload[i] !== FILL_EVENT_DISCRIMINATOR[i]) return null;
  }
  const body = payload.slice(EVENT_DISCRIMINATOR_LEN);
  const decoder = getFillEventDecoder();
  if (body.length < decoder.fixedSize) return null;
  try {
    return decoder.decode(body.slice(0, decoder.fixedSize));
  } catch {
    return null;
  }
}

/**
 * Assemble the transaction's full account-key list in the order an
 * instruction's `programIdIndex` addresses: the message's static keys first,
 * then the address-lookup-table loaded addresses (writable, then readonly).
 *
 * Assumes the transaction was fetched with `encoding: 'json'`, where
 * `accountKeys` holds only the *static* keys and the lookup-table addresses
 * arrive separately in `meta.loadedAddresses`. Under `jsonParsed` the key list
 * already includes the loaded addresses (as objects, not strings), so this
 * assembly would double-count — it fails closed there rather than crediting an
 * event to the wrong program, since an object never matches the program
 * address.
 *
 * Returns `null` when the static key list is missing — the caller then can't
 * safely attribute an event and skips the transaction rather than trust an
 * unverified emitter.
 */
export function eventAccountKeys(tx: EventTransaction): readonly string[] | null {
  const staticKeys = tx.transaction.message.accountKeys;
  if (!staticKeys) return null;
  const loaded = tx.meta?.loadedAddresses;
  if (!loaded) return staticKeys;
  // Tolerate a malformed `loadedAddresses` the declared type forbids: spreading
  // a non-array would throw out of a decoder whose every other bad-input path
  // returns null/[], so a hostile or buggy RPC could take out the caller.
  const writable = Array.isArray(loaded.writable) ? loaded.writable : [];
  const readonly = Array.isArray(loaded.readonly) ? loaded.readonly : [];
  return [...staticKeys, ...writable, ...readonly];
}

/**
 * Every {@link FillEvent} our program emitted in this transaction, in
 * inner-instruction order.
 *
 * Two conditions gate every event before its bytes are trusted, and both are
 * necessary:
 *
 * 1. **The transaction succeeded.** A failed transaction still carries the
 *    inner instructions recorded up to the point it failed, so its `meta`
 *    can hold event-shaped bytes that never took effect. That is reachable by
 *    an attacker: a foreign program can CPI into this program with
 *    `[tag][discriminator][fabricated body]`, and because inner instructions
 *    record the program being *invoked*, the recorded `programIdIndex` resolves
 *    to us and passes check 2. anchor rejects the call (the event-authority PDA
 *    can't sign for a foreign invoker) so the transaction fails — which is
 *    precisely why the failure has to be checked here rather than left to
 *    callers.
 * 2. **The emitting program is {@link DROPSET_PROGRAM_ADDRESS}.** The tag and
 *    the discriminator are both public values, so the emitting program id is
 *    the only thing `emit_cpi!`'s self-CPI actually authenticates.
 *
 * A transaction whose key list can't be resolved yields nothing rather than an
 * unattributed fill.
 */
export function collectFillEvents(tx: EventTransaction): FillEvent[] {
  if (tx.meta?.err != null) return [];
  const sets = tx.meta?.innerInstructions;
  if (!sets) return [];
  const accountKeys = eventAccountKeys(tx);
  if (!accountKeys) return [];

  const base58 = getBase58Encoder();
  const fills: FillEvent[] = [];
  for (const set of sets) {
    for (const instruction of set.instructions) {
      if (accountKeys[instruction.programIdIndex] !== DROPSET_PROGRAM_ADDRESS) {
        continue;
      }
      let data: ReadonlyUint8Array;
      try {
        data = base58.encode(instruction.data);
      } catch {
        continue;
      }
      const payload = stripEventTag(data);
      if (!payload) continue;
      const fill = decodeFillEventPayload(payload);
      if (fill) fills.push(fill);
    }
  }
  return fills;
}
