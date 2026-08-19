/**
 * The clock-domain guard, demonstrated and pinned.
 *
 * The behavioral half (saturation, zero-is-dead, the strict gate) is
 * ordinary assertions. The *typing* half is the point of the module,
 * though, and a type guard needs a test that fails when the guard stops
 * working — so each transposition the PR #310 review mutated in is
 * written out under `@ts-expect-error`.
 *
 * That directive is an assertion, not a suppression: `tsc` reports
 * "Unused '@ts-expect-error' directive" if the line below it ever
 * compiles cleanly. So if someone widens {@link SlotTime} back to a bare
 * `number`, or drops a brand, `pnpm exec tsc --noEmit` goes red here.
 *
 * The type checker is the assertion, but note the lines below **do** run:
 * they sit in a `node:test` callback and the brands are erased at
 * runtime, so `slotDeadlineAfter(slotTime(7), wallSpan(600))` really
 * executes. That is harmless while these factories are identity
 * functions, and worth knowing if one ever starts validating and
 * throwing.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  DEAD_SLOT_SPAN,
  DEAD_SLOT_TIME,
  DEAD_WALL_SPAN,
  DEAD_WALL_TIME,
  NO_SLOT_BOUND,
  NO_WALL_BOUND,
  rawClock,
  slotDeadlineAfter,
  slotIsLiveAt,
  slotSpan,
  type SlotTime,
  slotTime,
  wallDeadlineAfter,
  wallIsLiveAt,
  wallSpan,
  type WallTime,
  wallTime,
} from './clock';

// ── The guard ────────────────────────────────────────────────────────

test('a domain transposition does not type-check', () => {
  // Pairing a datum with the *other* domain's offset.
  // @ts-expect-error — a WallSpan is not a SlotSpan
  slotDeadlineAfter(slotTime(7), wallSpan(600));
  // @ts-expect-error — a SlotSpan is not a WallSpan
  wallDeadlineAfter(wallTime(1_700_000_000), slotSpan(50));

  // Gating a deadline against the other domain's "now".
  // @ts-expect-error — a SlotTime is not a WallTime
  wallIsLiveAt(wallTime(1_700_000_600), slotTime(57));
  // @ts-expect-error — a WallTime is not a SlotTime
  slotIsLiveAt(slotTime(57), wallTime(1_700_000_600));

  // Assigning across domains — the two datums transposed at the source.
  // @ts-expect-error — the brands are nominal, so this is not a widening
  const _slot: SlotTime = wallTime(1_700_000_000);
  // @ts-expect-error — and it is symmetric
  const _wall: WallTime = slotTime(7);

  // An untagged number cannot enter a domain by accident either: the
  // brand has to be applied deliberately, at a boundary.
  // @ts-expect-error — a bare number is not a SlotTime
  slotIsLiveAt(slotTime(57), 56);

  // The one axis TS canNOT guard, recorded so nobody assumes it does:
  // a relational compare of two spans from different domains type-checks
  // cleanly, because `<` only requires both operands be assignable to
  // `number` and a branded number is. Verified: putting
  // `// @ts-expect-error` above `slotSpan(120) < wallSpan(36)` makes tsc
  // report TS2578 "Unused directive". Rust's newtypes DO catch this (it
  // is the fourth `compile_fail` doctest in `dropset-math-core::clock`),
  // so the two guards are not at parity and the docs say so.
  assert.ok(true);

  // The assertion is the type check above; this keeps the runtime test
  // honest about having executed.
  assert.ok(true);
});

test('the honest form does type-check', () => {
  const wallDeadline = wallDeadlineAfter(wallTime(1_700_000_000), wallSpan(600));
  const slotDeadline = slotDeadlineAfter(slotTime(7), slotSpan(50));
  assert.equal(rawClock(wallDeadline), 1_700_000_600);
  assert.equal(rawClock(slotDeadline), 57);
  // A level rests only inside BOTH.
  assert.ok(wallIsLiveAt(wallDeadline, wallTime(1_700_000_599)));
  assert.ok(slotIsLiveAt(slotDeadline, slotTime(56)));
});

// ── The behavior, mirroring the Rust `clock::tests` ──────────────────

test('a zero span is dead whatever the datum', () => {
  // The whole point of the special case: a far-future datum must not
  // resurrect a level with no life in this domain. Written through the
  // named DEAD spans rather than a bare `slotSpan(0)` so the constants
  // the Rust mirror exercises (`SlotSpan::DEAD`) are exercised here too.
  assert.equal(wallDeadlineAfter(wallTime(0xffff_fffe), DEAD_WALL_SPAN), DEAD_WALL_TIME);
  assert.equal(slotDeadlineAfter(slotTime(1_000_000), DEAD_SLOT_SPAN), DEAD_SLOT_TIME);
});

test('the dead deadline is dead at every now', () => {
  assert.equal(wallIsLiveAt(DEAD_WALL_TIME, wallTime(0)), false);
  assert.equal(wallIsLiveAt(DEAD_WALL_TIME, wallTime(0xffff_ffff)), false);
});

test('a deadline saturates rather than wrapping', () => {
  const t = slotDeadlineAfter(slotTime(0xffff_ffff - 5), slotSpan(100));
  assert.equal(rawClock(t), 0xffff_ffff);
  assert.ok(slotIsLiveAt(t, slotTime(0xffff_fffe)));
});

test('the gate is strict at the boundary', () => {
  const deadline = wallDeadlineAfter(wallTime(1_000), wallSpan(50));
  assert.equal(rawClock(deadline), 1_050);
  assert.ok(wallIsLiveAt(deadline, wallTime(1_049)));
  // Exactly at the deadline is already dead — matches the engine's
  // `expires_at_unix <= now_unix` skip.
  assert.equal(wallIsLiveAt(deadline, wallTime(1_050)), false);
  assert.equal(wallIsLiveAt(deadline, wallTime(1_051)), false);
});

test('unbounded clears any reachable datum', () => {
  assert.equal(rawClock(slotDeadlineAfter(slotTime(0), NO_SLOT_BOUND)), 0xffff_ffff);
  assert.equal(rawClock(wallDeadlineAfter(wallTime(2_000_000_000), NO_WALL_BOUND)), 0xffff_ffff);
});

test('the two domains agree on how to spell unbounded', () => {
  // One named constant per domain, same value — the unification the
  // seeded demo ladders' bare `u32::MAX` used to break.
  assert.equal(rawClock(NO_SLOT_BOUND), rawClock(NO_WALL_BOUND));
  assert.equal(rawClock(NO_SLOT_BOUND), 0xffff_ffff);
});
