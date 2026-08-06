//! Bot configuration — every knob, with MVP defaults.
//!
//! Defaults give a benign, gently-clustering flow against a mock
//! `<token>/USDC` demo market: a quiet baseline punctuated by short bursts,
//! modest LogNormal
//! order sizes, and a roughly balanced buy/sell split. The optional
//! passive / retail / aggressive presets from dropset-alpha are deliberately
//! *not* a deliverable — re-parameterize [`FlowConfig`] to taste.
//!
//! Sizing has two independent limits, and they are deliberately layered. The
//! LogNormal parameters ([`FlowConfig::median_notional`] /
//! [`FlowConfig::size_log_sigma`]) shape the *absolute* take size, tuned so
//! even the tail sits inside the seeded book; [`BotConfig::max_depth_fraction`]
//! then caps each take against the book that is actually resting when it is
//! sent. The first keeps the flow realistic, the second keeps it in bounds when
//! the book is thinner than the parameters assumed — a drained vault, a wider
//! spread, a market seeded at a different scale.

use std::time::Duration;

/// Default localnet RPC endpoint (the `solana-test-validator` the TUI spawns).
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";

/// Lamports per SOL.
pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// The taker role key (`keys/README.md` → `FFFF`). Signs and pays for its own
/// swaps; funded by an airdrop on startup.
pub const DEFAULT_TAKER_KEY: &str = "keys/FFFF.json";

/// The mint-authority wallet — the localnet admin the bootstrap created the
/// mock mints under (`keys/README.md` → `BBBB`, the TUI's default wallet). Used
/// only to mint the taker its starting inventory; it never signs a swap.
/// Resolved cwd-relative like the other `keys/` roles, so it works unchanged in
/// the container (WORKDIR `/app`, `keys/` mounted at `/app/keys`).
pub const DEFAULT_MINT_AUTHORITY_KEY: &str = "keys/BBBB.json";

/// The stochastic order-flow parameters consumed by [`crate::model::Flow`].
#[derive(Clone, Debug)]
pub struct FlowConfig {
    /// RNG seed. `Some` ⇒ a reproducible flow (same seed replays identically);
    /// `None` ⇒ seed from OS entropy.
    pub seed: Option<u64>,

    /// Mean order arrivals per tick while `Quiet` (the Poisson intensity).
    pub lambda_quiet: f64,
    /// Mean order arrivals per tick while in a `Burst`.
    pub lambda_burst: f64,
    /// Per-tick probability of entering a burst from quiet.
    pub burst_entry_prob: f64,
    /// Per-tick probability of leaving a burst back to quiet (so the mean
    /// burst lasts `1 / burst_exit_prob` ticks).
    pub burst_exit_prob: f64,

    /// Median order size, as a quote notional (e.g. USDC). The LogNormal's
    /// `mu` is `ln(median_notional)`.
    pub median_notional: f64,
    /// LogNormal shape `sigma` — larger means a heavier right tail (more big
    /// orders relative to the median).
    pub size_log_sigma: f64,

    /// Initial `P(buy)` for the first order.
    pub buy_bias_init: f64,
    /// Fraction of the gap to 0.5 the buy-bias closes each order (the
    /// mean-reversion strength).
    pub buy_bias_reversion: f64,
    /// Standard deviation of the Gaussian shock applied to the buy-bias each
    /// order (the random-walk component). `0.0` ⇒ a deterministic revert.
    pub buy_bias_shock: f64,
    /// Lower / upper clamp on the buy-bias, keeping the side draw away from
    /// the degenerate always-buy / always-sell endpoints.
    pub buy_bias_min: f64,
    pub buy_bias_max: f64,
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            seed: None,
            // Frequent flow so the book visibly moves and the fills tape stays
            // busy: a quiet tick now averages ~0.8 orders and a burst ~4, with
            // bursts entered more readily.
            lambda_quiet: 0.8,
            lambda_burst: 4.0,
            burst_entry_prob: 0.1,
            burst_exit_prob: 0.3,
            // ~$4 median take — small relative to the ~$100 seeded book
            // (SEED_USD_PER_SIDE), so each take nibbles a level or two rather
            // than overrunning the book: many small fills, not a few skips. σ
            // is tight enough that the tail itself stays inside book depth
            // (~$19 at the 99.9th percentile against the ~$90 resting inside
            // the taker's slippage limit), which leaves
            // `BotConfig::max_depth_fraction` as the backstop for a depleted
            // book rather than the thing sizing most takes.
            median_notional: 4.0,
            size_log_sigma: 0.5,
            // Balanced, gently autocorrelated flow.
            buy_bias_init: 0.5,
            buy_bias_reversion: 0.1,
            buy_bias_shock: 0.05,
            buy_bias_min: 0.05,
            buy_bias_max: 0.95,
        }
    }
}

/// The full taker-bot configuration.
#[derive(Clone, Debug)]
pub struct BotConfig {
    /// RPC endpoint.
    pub rpc_url: String,
    /// Tick interval — one regime step and its order arrivals per tick.
    pub tick: Duration,

    /// The stochastic flow parameters.
    pub flow: FlowConfig,

    /// Slippage tolerance (fraction, e.g. `0.01` = 1%). Sets how far the
    /// swap's `limit_price_bits` sits past the simulated fill price and how
    /// far `min_out` sits below the simulated output, so a benign move
    /// between sizing and execution doesn't abort the take.
    pub slippage_tolerance: f64,

    /// The most of the live book one take may consume, as a fraction of the
    /// depth resting inside its limit price (e.g. `0.25` = a quarter). The
    /// sampled notional is an absolute quote size, so on a thin book its tail
    /// can clear several levels at once; capping against *live* depth keeps a
    /// take proportional to whatever the maker is actually quoting, at $100 or
    /// at $1M. A non-positive value disables the cap.
    pub max_depth_fraction: f64,

    /// SOL airdropped to the taker on startup (and topped up when low), in
    /// lamports.
    pub airdrop_lamports: u64,
    /// Top up the taker's SOL when its balance falls below this (lamports).
    pub min_taker_lamports: u64,

    /// Target per-leg inventory the taker is minted, in whole tokens. Each
    /// leg is refilled to this when it runs low so the bot trades
    /// indefinitely without manual funding.
    pub inventory_target_tokens: f64,
    /// Refill a leg when its balance falls below this (whole tokens).
    pub inventory_min_tokens: f64,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            rpc_url: DEFAULT_RPC_URL.to_string(),
            // A short tick so orders arrive often enough to keep the book moving
            // through a walkthrough (the flow's regime still clusters activity).
            tick: Duration::from_secs(2),
            flow: FlowConfig::default(),
            slippage_tolerance: 0.01,
            // A quarter of the depth inside the limit price. Against the
            // opening ladder that is ~half its top rung — a visible nibble
            // that still leaves the level standing, which is the flow the demo
            // wants to show.
            max_depth_fraction: 0.25,
            airdrop_lamports: 2 * LAMPORTS_PER_SOL,
            min_taker_lamports: LAMPORTS_PER_SOL / 2,
            inventory_target_tokens: 1_000_000.0,
            inventory_min_tokens: 100_000.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Burst intensity must exceed quiet intensity, else the regime split is
    /// meaningless.
    #[test]
    fn burst_is_more_active_than_quiet() {
        let f = FlowConfig::default();
        assert!(f.lambda_burst > f.lambda_quiet);
    }

    /// The buy-bias clamp brackets the neutral 0.5 start, so the default flow
    /// can drift either way around even.
    #[test]
    fn bias_bounds_bracket_neutral() {
        let f = FlowConfig::default();
        assert!(f.buy_bias_min < 0.5 && f.buy_bias_max > 0.5);
        assert!(f.buy_bias_min >= 0.0 && f.buy_bias_max <= 1.0);
    }

    /// Probabilities are well-formed and the refill threshold sits below the
    /// target it refills toward.
    #[test]
    fn thresholds_are_well_formed() {
        let c = BotConfig::default();
        assert!((0.0..=1.0).contains(&c.flow.burst_entry_prob));
        assert!((0.0..=1.0).contains(&c.flow.burst_exit_prob));
        assert!(c.slippage_tolerance > 0.0 && c.slippage_tolerance < 1.0);
        assert!(c.inventory_min_tokens < c.inventory_target_tokens);
        assert!(c.min_taker_lamports < c.airdrop_lamports);
    }

    /// The depth cap is a real fraction of the book — enabled (positive) and
    /// under 1.0, so a take always leaves depth behind it.
    #[test]
    fn depth_fraction_leaves_the_book_standing() {
        let c = BotConfig::default();
        assert!(c.max_depth_fraction > 0.0 && c.max_depth_fraction < 1.0);
    }

    /// The sampled tail stays inside the depth the cap allows, so the cap is a
    /// backstop rather than the primary sizer: at the 99.9th percentile of
    /// `LogNormal(ln median, σ)` — `median · e^(3.09σ)` — a take is still a
    /// modest slice of the ~$100-per-side seeded book (`SEED_USD_PER_SIDE`).
    #[test]
    fn size_tail_stays_within_the_seeded_book() {
        let f = FlowConfig::default();
        let p999 = f.median_notional * (3.09 * f.size_log_sigma).exp();
        assert!(
            p999 < 25.0,
            "99.9th-percentile take {p999} is too large a share of the seeded book",
        );
    }
}
