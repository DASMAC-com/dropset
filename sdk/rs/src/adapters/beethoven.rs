//! beethoven integration — `blueshift-gg/beethoven` (CPI composability).
//!
//! **This is intentionally documentation-only.** beethoven is not an
//! off-chain quoting adapter like the others in this module: it is a
//! Pinocchio-based, `no_std` **on-chain CPI routing** layer where each
//! action is a Rust trait with a `TryFrom<&[AccountView]>` account parser
//! and `execute` / `execute_signed` methods, detected by "first account ==
//! target program id". A Dropset integration is therefore a *swap action
//! handler* that CPIs into Dropset's `swap`.
//!
//! Per interface.md § 6, **CPIs do not live in this off-chain SDK** — they
//! belong in the separate on-chain `dropset-interface` crate (`no_std`,
//! entrypoint-free: instruction builders + account layouts + price-core).
//! That crate is where beethoven's swap-action trait should be implemented,
//! alongside the router CPI builders.
//!
//! ## Blocker (interface.md § 4)
//!
//! beethoven swap composability is **post-MVP, blocked on an upstream
//! swap-context extension** to Dropset's `swap`: a CPI taker needs a
//! **named owner + fallback** in the swap context (so the caller program,
//! not just a signing taker, can own the output ATAs). beethoven currently
//! ships `deposit` / `deposit_signed` for Kamino and Jupiter; a swap action
//! and the Dropset handler are the work to add. This is exactly the
//! "if anything is required at the instruction level, go back and fix the
//! implementation" path the ticket calls out — tracked as a follow-up
//! against the on-chain crate rather than this SDK.
