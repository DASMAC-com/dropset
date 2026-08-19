//! The two clock domains, typed apart.
//!
//! Level expiry is **dual-domain**: a level rests only while it is inside
//! both a slot deadline and a wall-clock deadline (architecture.md §
//! *Expiry — the dual gate*). Both domains are counted in `u32`, and
//! before this module they were counted in the *same* `u32` — a slot
//! count and a unix second sat side by side, positionally, in every
//! layout struct and every matcher signature across the kernel, the
//! off-chain mirror, and the TS port.
//!
//! That is a hazard the test suite could not see. Transposing the two
//! datums left the entire Rust and TS suites green until the fixtures
//! were given deliberately distinguished values, so the only thing
//! standing between the engine and a silent domain swap was a fixture
//! convention — which decays the first time someone writes a new test
//! with lazy values.
//!
//! This module makes the guard structural. Each domain gets two types:
//!
//! - a **time** ([`SlotTime`], [`WallTime`]) — an absolute point in that
//!   domain: the stamped datum, the taker's "now", and the materialized
//!   deadline are all this;
//! - a **span** ([`SlotSpan`], [`WallSpan`]) — a relative offset in that
//!   domain: a level's per-level TIF.
//!
//! The only arithmetic offered is [`SlotTime::deadline_after`] /
//! [`WallTime::deadline_after`] (`Time + Span -> Time`) and comparison
//! within a domain. That is deliberately the whole algebra: it is enough
//! for every call site the engine has, and it makes all four ways the two
//! domains were previously confusable — datum/datum, offset/offset,
//! now/now, and datum-paired-with-the-wrong-offset — a **compile error**
//! rather than a green test.
//!
//! **Zero cost, zero wire impact.** Every type is
//! `#[repr(transparent)]` over its `u32`, so it is layout-identical to
//! the value it replaces and the distinction exists only in the Rust type
//! checker. Nothing here reaches the account layout, the IDL, or the
//! stored bytes — the layout structs keep their alignment-1 pod wrappers
//! and hand these out through typed accessors. The TS mirror gets the
//! same distinction from branded types in `sdk/ts/src/clock.ts`.
//!
//! **The ASM path never sees these types**, and must not: the on-chain
//! fast path moves the two datums as a single fused `u64` load/store
//! precisely *because* they are adjacent same-width fields (see
//! architecture.md § *SetReferencePrice → ASM fast path*). The typing
//! here is a Rust/TS-only guard laid over a layout that is deliberately
//! pair-shaped for the assembly.
//!
//! # The guard, demonstrated
//!
//! Each block below is one of the four transpositions the PR #310 review
//! mutated in and found the suite blind to. They are `compile_fail`
//! doctests, so `cargo test` fails if any of them ever starts compiling
//! — the guard cannot rot back into a convention without a red build.
//!
//! Pairing a datum with the *other* domain's offset:
//!
//! ```compile_fail
//! use dropset_math_core::clock::{SlotTime, WallSpan};
//! let _ = SlotTime::new(7).deadline_after(WallSpan::new(600));
//! ```
//!
//! Gating a wall deadline against a slot "now":
//!
//! ```compile_fail
//! use dropset_math_core::clock::{SlotTime, WallTime};
//! let _ = WallTime::new(1_700_000_600).is_live_at(SlotTime::new(57));
//! ```
//!
//! Assigning across domains — the two datums transposed at the source:
//!
//! ```compile_fail
//! use dropset_math_core::clock::{SlotTime, WallTime};
//! let slot: SlotTime = WallTime::new(1_700_000_000);
//! ```
//!
//! Comparing the two domains' offsets against one another:
//!
//! ```compile_fail
//! use dropset_math_core::clock::{SlotSpan, WallSpan};
//! let _ = SlotSpan::new(120) < WallSpan::new(36);
//! ```
//!
//! What *does* compile is the honest form — each domain's datum with its
//! own offset, each deadline against its own clock:
//!
//! ```
//! use dropset_math_core::clock::{SlotSpan, SlotTime, WallSpan, WallTime};
//!
//! let wall_deadline = WallTime::new(1_700_000_000).deadline_after(WallSpan::new(600));
//! let slot_deadline = SlotTime::new(7).deadline_after(SlotSpan::new(50));
//! assert_eq!(wall_deadline, WallTime::new(1_700_000_600));
//! assert_eq!(slot_deadline, SlotTime::new(57));
//!
//! // A level rests only inside BOTH.
//! let live = wall_deadline.is_live_at(WallTime::new(1_700_000_599))
//!     && slot_deadline.is_live_at(SlotTime::new(56));
//! assert!(live);
//!
//! // Comparison WITHIN a domain is fine, and this line is load-bearing:
//! // it is the positive control for the fourth `compile_fail` above.
//! // Without it, dropping `PartialOrd` from the spans entirely would
//! // keep that doctest failing — for the wrong reason — and it would
//! // still report green.
//! assert!(SlotSpan::new(36) < SlotSpan::new(120));
//! ```
//!
//! Every symbol the negative blocks use also appears in the positive
//! block above, which must compile — so a rename, a dropped
//! constructor, or a misspelled method turns *that* red rather than
//! letting a negative pass vacuously.

use bytemuck::{Pod, Zeroable};

/// Generate one domain's `(Time, Span)` pair.
///
/// The two domains are structurally identical and deliberately share no
/// operations, so the definitions are generated rather than written twice
/// — a hand-written second copy is exactly the kind of drift this module
/// exists to prevent.
macro_rules! clock_domain {
    (
        $time:ident, $span:ident,
        $domain:literal, $unit:literal, $unbounded_doc:literal
    ) => {
        #[doc = concat!("An absolute point in the ", $domain, " domain, in ", $unit, ".")]
        ///
        /// Covers all three absolute roles the engine has in this domain:
        /// the leader-stamped reference datum, the taker's observed
        /// "now", and a level's materialized deadline. They are the same
        /// kind of quantity and are compared against one another, so they
        /// share one type; what they must never be is confused with the
        /// *other* domain's point, which is what this type prevents.
        #[repr(transparent)]
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Pod, Zeroable)]
        pub struct $time(u32);

        #[doc = concat!("A relative offset in the ", $domain, " domain, in ", $unit, ".")]
        ///
        /// A level's per-level time-in-force, measured from its domain's
        /// reference datum. **Zero is dead** — see
        #[doc = concat!("[`", stringify!($time), "::deadline_after`].")]
        #[repr(transparent)]
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Pod, Zeroable)]
        pub struct $span(u32);

        impl $time {
            /// The dead sentinel. A level carrying this deadline never
            /// matches, because the gate is `now < deadline` and no
            /// `now` is below zero.
            pub const DEAD: Self = Self(0);

            /// Wrap a raw `u32` read from the stored layout or a clock.
            ///
            /// This is the **domain boundary**: past it the value is
            /// typed, and a transposition is a compile error. Keep the
            /// call sites few and obvious — a decode, a sysvar read, an
            /// instruction argument — so the places where the guard can
            /// still be defeated stay countable.
            #[inline(always)]
            pub const fn new(raw: u32) -> Self {
                Self(raw)
            }

            /// The raw `u32`, for storing back into the layout or
            /// formatting. The other half of the domain boundary.
            #[inline(always)]
            pub const fn get(self) -> u32 {
                self.0
            }

            /// This domain's absolute deadline for a level whose offset
            /// is `span`: `self + span`, **saturating** — except that a
            /// zero span yields [`Self::DEAD`] rather than the bare
            /// datum.
            ///
            /// That special case is what makes "zero in either domain is
            /// dead" true independently of the datum. Without it a
            /// leader stamping a future datum would give a zero-life
            /// level a deadline still ahead of the clock and it would
            /// match. Folding the check in here — at flush time, which
            /// runs once per quote — also keeps the taker's per-level
            /// gate a single unconditional compare per domain rather
            /// than a compare plus a zero test.
            ///
            /// This is the **one** definition of that rule. The on-chain
            /// flush and the off-chain mirror both call it, so the two
            /// cannot drift on the sentinel handling — which is the
            /// other half of what makes the mirrors trustworthy.
            #[inline(always)]
            pub const fn deadline_after(self, span: $span) -> Self {
                if span.0 == 0 {
                    return Self::DEAD;
                }
                Self(self.0.saturating_add(span.0))
            }

            /// Whether a level carrying this deadline is still live at
            /// `now` — the domain's half of the dual gate, `now <
            /// deadline`. Strict, so a deadline exactly at `now` is
            /// already dead, and [`Self::DEAD`] is dead at every `now`.
            #[inline(always)]
            pub const fn is_live_at(self, now: Self) -> bool {
                now.0 < self.0
            }
        }

        impl $span {
            #[doc = $unbounded_doc]
            ///
            /// Expressed as the **maximum** offset rather than a reserved
            /// sentinel, so the match gate stays one unconditional
            /// compare per domain rather than a compare plus a sentinel
            /// test.
            pub const UNBOUNDED: Self = Self(u32::MAX);

            /// The dead offset — a level with no life in this domain at
            /// all, whatever its datum.
            pub const DEAD: Self = Self(0);

            /// Wrap a raw `u32` offset. See
            #[doc = concat!("[`", stringify!($time), "::new`]")]
            /// on keeping the boundary narrow.
            #[inline(always)]
            pub const fn new(raw: u32) -> Self {
                Self(raw)
            }

            /// The raw `u32`, for storing back into the layout.
            #[inline(always)]
            pub const fn get(self) -> u32 {
                self.0
            }
        }

        impl core::fmt::Debug for $time {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!(stringify!($time), "({})"), self.0)
            }
        }

        impl core::fmt::Debug for $span {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                if *self == Self::UNBOUNDED {
                    return write!(f, concat!(stringify!($span), "(UNBOUNDED)"));
                }
                write!(f, concat!(stringify!($span), "({})"), self.0)
            }
        }
    };
}

clock_domain!(
    SlotTime,
    SlotSpan,
    "slot",
    "slots",
    "The offset meaning **no slot bound** — a level bounded only by its \
     wall TIF.\n\nThe `u32` ceiling is ~4.3e9 slots, decades of chain \
     time even at the fastest proposed slot durations, so it clears the \
     longest wall TIF any tier policy would set by a wide margin — \
     \"unbounded\" really is unbounded in every reachable regime."
);

clock_domain!(
    WallTime,
    WallSpan,
    "wall-clock",
    "unix seconds",
    "The offset meaning **no wall bound** — a level bounded only by its \
     slot TIF.\n\nThe counterpart to [`SlotSpan::UNBOUNDED`], and the \
     single spelling for it: the seeded demo ladders previously wrote a \
     bare `u32::MAX` here while the slot domain had a named constant, so \
     the two domains disagreed on how to say the same thing."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_span_is_dead_whatever_the_datum() {
        // The whole point of the special case: a far-future datum must
        // not resurrect a level with no life in this domain.
        let far_future = WallTime::new(u32::MAX - 1);
        assert_eq!(far_future.deadline_after(WallSpan::DEAD), WallTime::DEAD);
        assert_eq!(
            SlotTime::new(1_000_000).deadline_after(SlotSpan::DEAD),
            SlotTime::DEAD
        );
    }

    #[test]
    fn dead_deadline_is_dead_at_every_now() {
        assert!(!WallTime::DEAD.is_live_at(WallTime::new(0)));
        assert!(!WallTime::DEAD.is_live_at(WallTime::new(u32::MAX)));
    }

    #[test]
    fn deadline_saturates_rather_than_wrapping() {
        let t = SlotTime::new(u32::MAX - 5);
        assert_eq!(
            t.deadline_after(SlotSpan::new(100)),
            SlotTime::new(u32::MAX)
        );
        // Saturated, so still live at any representable `now` below it.
        assert!(t
            .deadline_after(SlotSpan::new(100))
            .is_live_at(SlotTime::new(u32::MAX - 1)));
    }

    #[test]
    fn gate_is_strict_at_the_boundary() {
        let deadline = WallTime::new(1_000).deadline_after(WallSpan::new(50));
        assert_eq!(deadline, WallTime::new(1_050));
        assert!(deadline.is_live_at(WallTime::new(1_049)));
        // Exactly at the deadline is already dead — matches the engine's
        // `expires_at_unix <= now_unix` skip.
        assert!(!deadline.is_live_at(WallTime::new(1_050)));
        assert!(!deadline.is_live_at(WallTime::new(1_051)));
    }

    #[test]
    fn unbounded_clears_any_reachable_datum() {
        // An unbounded span pins the deadline at the ceiling from any
        // datum, so the domain never binds.
        assert_eq!(
            SlotTime::new(0).deadline_after(SlotSpan::UNBOUNDED),
            SlotTime::new(u32::MAX)
        );
        assert_eq!(
            WallTime::new(2_000_000_000).deadline_after(WallSpan::UNBOUNDED),
            WallTime::new(u32::MAX)
        );
    }

    #[test]
    fn transparent_over_u32() {
        assert_eq!(core::mem::size_of::<SlotTime>(), 4);
        assert_eq!(core::mem::size_of::<SlotSpan>(), 4);
        assert_eq!(core::mem::size_of::<WallTime>(), 4);
        assert_eq!(core::mem::size_of::<WallSpan>(), 4);
        assert_eq!(
            core::mem::align_of::<WallTime>(),
            core::mem::align_of::<u32>()
        );
    }

    #[test]
    fn round_trips_through_the_domain_boundary() {
        assert_eq!(SlotTime::new(1_234).get(), 1_234);
        assert_eq!(WallSpan::new(u32::MAX).get(), u32::MAX);
        assert_eq!(WallSpan::UNBOUNDED.get(), u32::MAX);
        assert_eq!(SlotSpan::DEAD.get(), 0);
    }
}
