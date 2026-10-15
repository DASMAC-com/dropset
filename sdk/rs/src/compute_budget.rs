//! The compute-unit limit a client sets on a single-instruction `swap`
//! transaction.
//!
//! With no `SetComputeUnitLimit`, the runtime grants 200k CU per non-builtin
//! instruction, shared across the transaction. A transaction that is a bare
//! `swap` therefore gets 200k. On the default-cap book
//! (`registry.max_vaults_per_market = 10`) every take that fills without
//! stranding dust fits that; a take whose remaining input reaches dust can
//! overrun it, because a leg whose round trip floors to zero output is
//! skipped rather than ending the walk, so the take visits every entry left
//! on the book.
//!
//! Measured on the ten-vault book in `programs/dropset/tests/flip.rs`, which
//! asserts the first two figures stay under their bounds:
//!
//! - the costliest take that fills without stranding dust: a cold book
//!   (every vault flushed in the same transaction) taken to the heap
//!   ceiling, sixteen legs, about 178k CU on a sell and 181k on a buy,
//!   under the 200k a bare `swap` gets;
//! - a one-atom sell on a fully flipped book (160 entries walked, nothing
//!   filled): about 390k CU, under [`SWAP_COMPUTE_UNIT_LIMIT`];
//! - a take that fills to the heap ceiling and is then stranded with one
//!   atom pays both, an **estimated** 470k. It needs a last leg whose level
//!   cap lands just so, which the measured book never produced, so no test
//!   pins it; it is the case the headroom above 390k is for.
//!
//! Raising the vault cap scales all of these, by up to sixteen entries a
//! vault, so a cap change re-runs those tests first.
//!
//! The limit does **not** lift the heap ceiling: on that book a take
//! completes sixteen legs and a seventeenth aborts on the 32 KiB heap,
//! whatever its compute budget, so a client must not assume a deeper sweep
//! fits one transaction.
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
