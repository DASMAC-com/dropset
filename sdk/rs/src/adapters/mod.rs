//! Router / aggregator integration adapters.
//!
//! Each adapter maps a vendor's quoting + swap-CPI contract onto the
//! Dropset SDK: book state via [`crate::layout`], quotes via
//! [`crate::matching::simulate_swap`], and swap instructions via the
//! generated [`crate::instructions`] builders. See interface.md § 4.

pub mod dflow;
