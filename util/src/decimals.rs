//! The decimal-gap conversion between a **human** quote-per-base price (USD
//! per token, as the feeds and the UI speak it) and the **atoms-ratio** the
//! on-chain `Price` encodes (`quote_atoms` per `base_atoms`).
//!
//! The two coincide only when both legs share a decimal count, which is why
//! every 6-vs-6 pair on the demo roster hides a missing conversion: TGBP (9
//! decimals) against USDC (6) is where it shows, as a clean factor of 1000.
//! A token with more decimals than the quote scales the ratio down, fewer
//! scales it up.
//!
//! This lived open-coded in four places — the maker's stamp path, the TUI's
//! config range check, the TUI's re-peg success line, and the TUI's book
//! formatter. A fifth copy, in the markets pane, had grown its own resolution
//! bug the other four did not share, and was folded into the book formatter
//! before this module existed. That is the failure mode the crate exists to
//! stop: an open-coded conversion does not merely duplicate, it duplicates the
//! defect into files nobody is reading.
//!
//! # Why the two-power association is load-bearing
//!
//! Both functions scale by `10^a / 10^b` rather than the algebraically equal
//! `10^(a-b)`. That is deliberate and must not be "simplified":
//!
//! * `powi` is not correctly rounded — it is repeated multiplication — so
//!   `10f64.powi(-3)` can sit an ulp away from `1e-3`, whereas `10^6 / 10^9`
//!   divides two exactly-representable powers and rounds once.
//! * The frontend's `humanPrice`
//!   (`frontend/components/orderbook/format.ts`) is pinned to this exact
//!   grouping so the web pane and the TUI pane cannot disagree by an ulp on
//!   the same level. That pin is a cross-language contract these two
//!   functions are the Rust half of; changing the association here silently
//!   breaks it, and no Rust test would notice.
//!
//! Both take mint decimals as `u8` and assume the sane SPL range (0–12);
//! nothing on the roster approaches the exponent at which `10^n` overflows.

/// Convert a human quote-per-base price into the atoms-ratio the on-chain
/// `Price` encodes. This is the stamping direction — what the maker converts a
/// feed price through before `Price::from_value`, and what the TUI's config
/// range check measures each configured pair against.
///
/// See the module docs for why the scaling is written as two powers.
pub fn human_to_atoms_ratio(human: f64, base_decimals: u8, quote_decimals: u8) -> f64 {
    human * 10f64.powi(quote_decimals as i32) / 10f64.powi(base_decimals as i32)
}

/// Inverse of [`human_to_atoms_ratio`] — decode an on-chain atoms-ratio back
/// to the human quote-per-base price for display.
///
/// See the module docs for why the scaling is written as two powers.
pub fn atoms_ratio_to_human(ratio: f64, base_decimals: u8, quote_decimals: u8) -> f64 {
    ratio * 10f64.powi(base_decimals as i32) / 10f64.powi(quote_decimals as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoms_ratio_is_identity_at_equal_decimals() {
        // EURC (6) / USDC (6): the human price stamps unchanged.
        assert!((human_to_atoms_ratio(1.14, 6, 6) - 1.14).abs() < 1e-12);
    }

    #[test]
    fn atoms_ratio_scales_with_the_decimal_gap() {
        // VCHF (9) / USDC (6): 1 VCHF-atom is 10^-3 of a token, so the
        // atoms-ratio is the human price × 10^(6-9).
        assert!((human_to_atoms_ratio(1.235, 9, 6) - 1.235e-3).abs() < 1e-12);
        // IDRX (2) / USDC (6): the atoms-ratio scales up.
        assert!((human_to_atoms_ratio(0.000056, 2, 6) - 0.56).abs() < 1e-12);
    }

    #[test]
    fn atoms_ratio_round_trips_to_human() {
        for (human, base, quote) in [(1.14, 6, 6), (1.235, 9, 6), (0.000056, 2, 6)] {
            let ratio = human_to_atoms_ratio(human, base, quote);
            let back = atoms_ratio_to_human(ratio, base, quote);
            assert!((back - human).abs() / human < 1e-12, "round-trip {human}");
        }
    }

    /// Pin the two-power association itself, not just the value: this is the
    /// grouping the frontend's `humanPrice` mirrors, and collapsing it to a
    /// single `powi` is the plausible-looking edit that would break that
    /// cross-language agreement without failing anything else here.
    #[test]
    fn the_scaling_is_grouped_as_two_powers() {
        for (base, quote) in [(6u8, 6u8), (9, 6), (2, 6), (6, 9), (8, 6)] {
            let (b, q) = (base as i32, quote as i32);
            let ratio = 1.234_567_891_234_5_f64;

            assert_eq!(
                atoms_ratio_to_human(ratio, base, quote),
                ratio * 10f64.powi(b) / 10f64.powi(q),
                "inverse association drifted at {base}/{quote}"
            );
            assert_eq!(
                human_to_atoms_ratio(ratio, base, quote),
                ratio * 10f64.powi(q) / 10f64.powi(b),
                "forward association drifted at {base}/{quote}"
            );
        }
    }
}
