//! The compute-unit limit a client sets on a single-instruction `swap`
//! transaction.
//!
//! With no `SetComputeUnitLimit`, the runtime grants 200k CU per non-builtin
//! instruction, shared across the transaction. A transaction that is a bare
//! `swap` therefore gets 200k. On the default-cap book
//! (`registry.max_vaults_per_market = 10`) every take fits that, but with
//! little room: a take ends its walk once its remaining input is dust (one
//! input atom that prices to zero at the next level), so its cost is bounded
//! by the legs it fills, and the heap bounds those.
//!
//! Measured on the ten-vault book in `programs/dropset/tests/flip.rs`, which
//! asserts each stays under the 200k a bare `swap` gets:
//!
//! - the costliest take: a cold book (every vault flushed in the same
//!   transaction) taken to the heap ceiling, sixteen legs, with or without a
//!   one-atom remainder left over, about 184k CU on a sell and 185k on a buy;
//! - a one-atom sell on a fully flipped book (160 entries on the side, nothing
//!   filled): about 76k CU, since the walk ends at the first entry.
//!
//! [`SWAP_COMPUTE_UNIT_LIMIT`] sits about 60% over the costliest take, as
//! headroom for drift: a priority fee is charged on the requested limit, not
//! the units consumed, so a limit far above the worst case overpays on every
//! swap. Raising the vault cap scales the flush and the walk, by up to
//! sixteen entries a vault, so a cap change re-runs those tests first.
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
pub const SWAP_COMPUTE_UNIT_LIMIT: u32 = 300_000;
