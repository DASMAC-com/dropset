//! The compute-unit limit a client sets on a single-instruction `swap`
//! transaction.
//!
//! With no `SetComputeUnitLimit`, the runtime grants 200k CU per non-builtin
//! instruction, shared across the transaction. A transaction that is a bare
//! `swap` therefore gets 200k. A take's cost is the entries it visits:
//! the legs it fills, at most `MAX_LEGS_PER_SWAP`, plus the entries it skips at
//! about 2.4k CU each. A take ends its walk once its own remaining input is
//! dust (one input atom that prices to zero at the next level), so a dust
//! taker no longer walks the book. An entry is still skipped when its level
//! or vault is empty, or when its level or vault cap is so small the leg
//! prices to zero input, since the taker keeps budget a later level may
//! fill. A book quoted with such dust-sized levels throughout can still cost
//! a no-fill walk over every entry: roughly the 404k a one-atom sell walking
//! the fully flipped ten-vault book cost before a dust taker's walk ended,
//! an estimate never re-measured on a dust-quoted book.
//!
//! Measured on the ten-vault book in `programs/dropset/tests/flip.rs`, whose
//! levels are not dust-sized:
//!
//! - the costliest take: a cold book (every vault flushed in the same
//!   transaction) taken to the leg cap, twenty-two legs, with or without a
//!   one-atom remainder left over or a budget for the whole book, about 240k
//!   to 255k CU (it varies with the keys a run derives) — over the 200k a
//!   bare `swap` gets, which covers the first fourteen or fifteen legs (the
//!   test pins at least fourteen), and under [`SWAP_COMPUTE_UNIT_LIMIT`]. A
//!   remainder stranded short of the cap adds no walk, since it ends the
//!   take, and the cap ends a whole-book take: both complete all twenty-two
//!   legs, which the test pins, within the same range;
//! - a one-atom sell on a fully flipped book (160 entries, nothing filled):
//!   about 70k CU, since the walk ends at the first entry.
//!
//! [`SWAP_COMPUTE_UNIT_LIMIT`] covers the dust-quoted full-book walk above
//! as well, so a bare `swap` into such a book completes empty rather than
//! failing. A priority fee is charged on the requested limit, not the units
//! consumed, so the gap between this limit and an ordinary take is paid on
//! every swap; sizing the limit per take from simulation is the planned
//! fix. Raising the vault cap scales the flush and the walk, by up to
//! sixteen entries a vault, so a cap change re-runs those tests first.
//!
//! The limit does **not** lift the leg cap: a take fills at most
//! `MAX_LEGS_PER_SWAP` legs whatever its compute budget, and keeps the
//! budget it could not spend past them, so a client sweeping deeper sends
//! another swap rather than assuming one transaction clears the book.
//!
//! It binds the taker bot and the TUI's swap probe, which each send a bare
//! `swap`. Two paths set no limit on purpose. The frontend leaves the budget
//! to the wallet, whose priority fee a preset limit would suppress, and its
//! transaction carries two ATA creates beside the swap, so it gets 600k
//! regardless. A routed swap is an aggregator's: the [`crate::adapters`]
//! hand it one swap instruction inside a transaction it composes and
//! budgets.

/// The `SetComputeUnitLimit` a client sends ahead of a bare `swap`. See the
/// module docs for the measurement it is sized from and which clients it
/// binds.
pub const SWAP_COMPUTE_UNIT_LIMIT: u32 = 600_000;
