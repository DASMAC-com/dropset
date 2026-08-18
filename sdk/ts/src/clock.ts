/**
 * The two clock domains, typed apart (TypeScript mirror of
 * `dropset-math-core`'s `clock` module).
 *
 * Level expiry is **dual-domain**: a level rests only while it is inside
 * both a slot deadline and a wall-clock deadline. Both domains are
 * counted in a `u32`, and before this module they were counted in the
 * *same* `number` — a slot count and a unix second sat side by side,
 * positionally, in every decoded struct and every matcher signature here
 * and in the two Rust forks.
 *
 * That is a hazard the test suite could not see. Transposing the two
 * datums left the entire Rust and TS suites green until the fixtures were
 * given deliberately distinguished values, so the only thing standing
 * between the book reader and a silent domain swap was a fixture
 * convention — which decays the first time someone writes a new test with
 * lazy values.
 *
 * Each domain gets two branded types:
 *
 * - a **time** ({@link SlotTime}, {@link WallTime}) — an absolute point:
 *   the stamped datum, the caller's "now", and a materialized deadline
 *   are all this;
 * - a **span** ({@link SlotSpan}, {@link WallSpan}) — a relative offset
 *   in that domain: a level's per-level TIF.
 *
 * **Zero runtime cost.** A branded type is `number & { readonly [tag]:
 * true }`: the brand is a phantom property that exists only in the type
 * checker, so every value here *is* the number it wraps and the emitted
 * JS is unchanged. Nothing about the wire format, the decoded bytes, or
 * the WASM boundary moves.
 *
 * The brands are nominal, so `slotTime(1)` is not assignable to
 * {@link WallTime} and vice versa — which is the whole point. Crossing a
 * domain is deliberate and explicit: unwrap with {@link rawClock} and
 * re-wrap.
 */

declare const slotTimeBrand: unique symbol;
declare const slotSpanBrand: unique symbol;
declare const wallTimeBrand: unique symbol;
declare const wallSpanBrand: unique symbol;

/** An absolute point in the **slot** domain, in slots. */
export type SlotTime = number & { readonly [slotTimeBrand]: true };
/** A relative offset in the **slot** domain, in slots. Zero is dead. */
export type SlotSpan = number & { readonly [slotSpanBrand]: true };
/** An absolute point in the **wall-clock** domain, in unix seconds. */
export type WallTime = number & { readonly [wallTimeBrand]: true };
/** A relative offset in the **wall-clock** domain, in unix seconds. Zero is dead. */
export type WallSpan = number & { readonly [wallSpanBrand]: true };

/** Any of the four clock-domain types — the argument {@link rawClock} takes. */
export type ClockValue = SlotTime | SlotSpan | WallTime | WallSpan;

/**
 * Tag a raw slot number. This is the **domain boundary**: past it the
 * value carries its domain in the type, and a transposition is a compile
 * error. Keep the call sites few and obvious — a decode, an RPC read —
 * so the places where the guard can still be defeated stay countable.
 */
export const slotTime = (raw: number): SlotTime => raw as SlotTime;
/** Tag a raw slot offset. See {@link slotTime}. */
export const slotSpan = (raw: number): SlotSpan => raw as SlotSpan;
/** Tag a raw unix-second number. See {@link slotTime}. */
export const wallTime = (raw: number): WallTime => raw as WallTime;
/** Tag a raw wall-clock offset. See {@link slotTime}. */
export const wallSpan = (raw: number): WallSpan => raw as WallSpan;

/**
 * Strip the domain brand back to a plain `number`, for formatting, JSON,
 * or the WASM boundary — the other half of the domain boundary.
 */
export const rawClock = (v: ClockValue): number => v;

/**
 * The dead sentinel in either time domain. A level carrying this
 * deadline never matches: the gate is `now < deadline`, and no `now` is
 * below zero.
 */
export const DEAD_SLOT_TIME = slotTime(0);
/** @see DEAD_SLOT_TIME */
export const DEAD_WALL_TIME = wallTime(0);

/** A level with no life in the slot domain at all, whatever its datum. */
export const DEAD_SLOT_SPAN = slotSpan(0);
/** A level with no life in the wall domain at all, whatever its datum. */
export const DEAD_WALL_SPAN = wallSpan(0);

/**
 * The offset meaning **no slot bound** — a level bounded only by its
 * wall TIF. Mirrors the Rust `SlotSpan::UNBOUNDED`.
 *
 * Expressed as the maximum offset rather than a reserved sentinel, so
 * the match gate stays one unconditional compare per domain. The `u32`
 * ceiling is ~4.3e9 slots, decades of chain time even at the fastest
 * proposed slot durations, so it clears the longest wall TIF any tier
 * policy would set by a wide margin.
 */
export const NO_SLOT_BOUND = slotSpan(0xffff_ffff);

/**
 * The offset meaning **no wall bound** — a level bounded only by its
 * slot TIF. Mirrors the Rust `WallSpan::UNBOUNDED`.
 *
 * The counterpart to {@link NO_SLOT_BOUND}, and the single spelling for
 * it: the seeded demo ladders previously wrote a bare `u32::MAX` here
 * while the slot domain had a named constant, so the two domains
 * disagreed on how to say the same thing.
 */
export const NO_WALL_BOUND = wallSpan(0xffff_ffff);

/** The `u32` ceiling both domains saturate at. */
const U32_MAX = 0xffff_ffff;

/**
 * The slot domain's absolute deadline for a level whose offset is
 * `span`: `datum + span`, **saturating** — except that a zero span
 * yields {@link DEAD_SLOT_TIME} rather than the bare datum.
 *
 * That special case is what makes "zero in either domain is dead" true
 * independently of the datum. Without it a leader stamping a future
 * datum would give a zero-life level a deadline still ahead of the clock
 * and it would match.
 */
export function slotDeadlineAfter(datum: SlotTime, span: SlotSpan): SlotTime {
  return span === 0 ? DEAD_SLOT_TIME : slotTime(Math.min(datum + span, U32_MAX));
}

/** The wall domain's absolute deadline. @see slotDeadlineAfter */
export function wallDeadlineAfter(datum: WallTime, span: WallSpan): WallTime {
  return span === 0 ? DEAD_WALL_TIME : wallTime(Math.min(datum + span, U32_MAX));
}

/**
 * Whether a level carrying `deadline` is still live at `now` — one
 * domain's half of the dual gate. Strict, so a deadline exactly at `now`
 * is already dead, and the dead sentinel is dead at every `now`.
 */
export function slotIsLiveAt(deadline: SlotTime, now: SlotTime): boolean {
  return now < deadline;
}

/** The wall domain's half of the dual gate. @see slotIsLiveAt */
export function wallIsLiveAt(deadline: WallTime, now: WallTime): boolean {
  return now < deadline;
}
