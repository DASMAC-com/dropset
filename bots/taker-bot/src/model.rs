//! The pure, seedable stochastic order-flow process.
//!
//! Models dropset-alpha's taker (`services/taker-bot/src/taker.rs`) as three
//! composable layers:
//!
//! 1. **Regime** — a two-state Markov chain over {quiet, burst}. Each tick it
//!    transitions with the configured entry / exit probabilities, and its
//!    state selects the arrival intensity `λ` for that tick.
//! 2. **Arrivals** — the number of orders this tick is `Poisson(λ)`, so bursts
//!    cluster naturally and quiet ticks often produce nothing.
//! 3. **Per-order** — each order's notional size is `LogNormal` (heavy right
//!    tail: many small takes, occasional large ones), and its side is a coin
//!    weighted by a `buy_bias` that mean-reverts toward 0.5 with a Gaussian
//!    shock each order — giving autocorrelated runs of buys or sells that
//!    revert rather than a fixed 50/50 split.
//!
//! Everything here is deterministic given the seed: same seed ⇒ same flow,
//! which is what makes `--dry-run` a faithful preview of the order flow and
//! the unit tests reproducible. Sizes are emitted as a **quote-denominated
//! notional** (a float in human units, e.g. USDC); turning that into an
//! `amount_in` for a given side and into the swap's `min_out` is the chain
//! layer's job ([`crate::chain`]), so this module stays free of decimals,
//! prices, and the book.

// cspell:word rngs

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, LogNormal, Poisson};

use crate::config::FlowConfig;
use dropset_sdk::matching::SwapSide;

/// The arrival-intensity regime. The chain spends most of its time `Quiet`
/// and occasionally enters a `Burst` of elevated activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    Quiet,
    Burst,
}

/// One sampled order: which side to take and how large, as a quote-
/// denominated notional (human units, e.g. USDC). The chain layer converts
/// the notional to an `amount_in` in atoms for the chosen input leg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Order {
    pub side: SwapSide,
    /// Order size as a quote notional (e.g. USDC), always `> 0`.
    pub notional: f64,
}

/// The stochastic flow generator: the RNG plus the mutable process state
/// (current regime and buy-bias) and the immutable parameters.
pub struct Flow {
    rng: StdRng,
    cfg: FlowConfig,
    regime: Regime,
    /// Current `P(buy)` for the next order, in `(0, 1)`.
    buy_bias: f64,
}

impl Flow {
    /// Build a flow from `cfg`. Seeded from `cfg.seed` when set (reproducible),
    /// else from OS entropy. Starts `Quiet` at the configured initial bias.
    pub fn new(cfg: FlowConfig) -> Self {
        let rng = match cfg.seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::from_entropy(),
        };
        let buy_bias = cfg.buy_bias_init;
        Self {
            rng,
            cfg,
            regime: Regime::Quiet,
            buy_bias,
        }
    }

    /// The current regime (for logging / inspection).
    pub fn regime(&self) -> Regime {
        self.regime
    }

    /// The current buy-bias (for logging / inspection).
    pub fn buy_bias(&self) -> f64 {
        self.buy_bias
    }

    /// Advance the regime Markov chain one tick and return the arrival
    /// intensity `λ` now in force.
    fn step_regime(&mut self) -> f64 {
        self.regime = match self.regime {
            Regime::Quiet if self.rng.gen_bool(self.cfg.burst_entry_prob) => Regime::Burst,
            Regime::Burst if self.rng.gen_bool(self.cfg.burst_exit_prob) => Regime::Quiet,
            same => same,
        };
        match self.regime {
            Regime::Quiet => self.cfg.lambda_quiet,
            Regime::Burst => self.cfg.lambda_burst,
        }
    }

    /// Sample the number of order arrivals for an intensity `λ`. A non-positive
    /// `λ` yields no arrivals (`Poisson` requires `λ > 0`).
    fn arrivals(&mut self, lambda: f64) -> u32 {
        if lambda <= 0.0 {
            return 0;
        }
        // `Poisson::new` only fails for a non-finite / non-positive `λ`, both
        // excluded above, so the sample is always defined.
        let n = Poisson::new(lambda)
            .expect("lambda > 0 and finite")
            .sample(&mut self.rng);
        n as u32
    }

    /// Sample one order: its side from the current buy-bias, then nudge the
    /// bias back toward 0.5 with a Gaussian shock (so flow is autocorrelated
    /// but mean-reverting), and its notional from the LogNormal size law.
    fn next_order(&mut self) -> Order {
        let side = if self.rng.gen::<f64>() < self.buy_bias {
            SwapSide::Buy
        } else {
            SwapSide::Sell
        };
        self.step_buy_bias();
        Order {
            side,
            notional: self.sample_notional(),
        }
    }

    /// Mean-revert the buy-bias toward 0.5 by `reversion` of the gap, add a
    /// Gaussian shock, and clamp away from the degenerate 0 / 1 endpoints.
    fn step_buy_bias(&mut self) {
        let reverted = self.buy_bias + self.cfg.buy_bias_reversion * (0.5 - self.buy_bias);
        let shock = sample_gaussian(&mut self.rng, self.cfg.buy_bias_shock);
        self.buy_bias = (reverted + shock).clamp(self.cfg.buy_bias_min, self.cfg.buy_bias_max);
    }

    /// Draw a positive notional from `LogNormal(ln(median), sigma)`, so the
    /// median is `median_notional` and `sigma` sets the spread.
    fn sample_notional(&mut self) -> f64 {
        let mu = self.cfg.median_notional.max(f64::MIN_POSITIVE).ln();
        // `LogNormal::new` only fails on a non-finite parameter; `mu` is finite
        // and `sigma` is a checked-positive config value.
        LogNormal::new(mu, self.cfg.size_log_sigma)
            .expect("finite LogNormal parameters")
            .sample(&mut self.rng)
    }

    /// Advance one tick: step the regime, then sample that tick's orders. May
    /// be empty (a quiet tick with zero arrivals).
    pub fn tick(&mut self) -> Vec<Order> {
        let lambda = self.step_regime();
        let n = self.arrivals(lambda);
        (0..n).map(|_| self.next_order()).collect()
    }
}

/// Sample `N(0, sigma)` from a uniform RNG via the Box–Muller transform, so
/// the buy-bias shock needs no extra distribution dependency. `sigma == 0`
/// yields exactly 0 (a deterministic, shock-free walk).
fn sample_gaussian(rng: &mut StdRng, sigma: f64) -> f64 {
    if sigma <= 0.0 {
        return 0.0;
    }
    // Guard the log against u1 == 0; (0, 1] keeps it finite.
    let u1: f64 = 1.0 - rng.gen::<f64>();
    let u2: f64 = rng.gen::<f64>();
    let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
    z * sigma
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed-seed config with a known shape for distributional assertions.
    fn cfg() -> FlowConfig {
        FlowConfig {
            seed: Some(42),
            ..FlowConfig::default()
        }
    }

    /// Same seed ⇒ identical flow, tick for tick. This is what lets
    /// `--dry-run` preview a live run and keeps these tests reproducible.
    #[test]
    fn seed_replays_identically() {
        let mut a = Flow::new(cfg());
        let mut b = Flow::new(cfg());
        for _ in 0..200 {
            assert_eq!(a.tick(), b.tick());
        }
    }

    /// Over many ticks the mean arrival count lands between the quiet and
    /// burst intensities — the chain visits both regimes, not just one.
    #[test]
    fn arrival_rate_is_between_the_two_lambdas() {
        let c = cfg();
        let (lo, hi) = (c.lambda_quiet, c.lambda_burst);
        let mut flow = Flow::new(c);
        let ticks = 20_000;
        let total: u32 = (0..ticks).map(|_| flow.tick().len() as u32).sum();
        let mean = total as f64 / ticks as f64;
        assert!(mean > lo, "mean {mean} should exceed quiet λ {lo}");
        assert!(mean < hi, "mean {mean} should be under burst λ {hi}");
    }

    /// The regime actually enters and leaves the burst state — a single-state
    /// chain would generate the wrong clustering.
    #[test]
    fn regime_visits_both_states() {
        let mut flow = Flow::new(cfg());
        let mut seen_quiet = false;
        let mut seen_burst = false;
        for _ in 0..5_000 {
            flow.tick();
            match flow.regime() {
                Regime::Quiet => seen_quiet = true,
                Regime::Burst => seen_burst = true,
            }
        }
        assert!(seen_quiet && seen_burst);
    }

    /// With the bias initialized at 0.5 and symmetric dynamics, the long-run
    /// buy fraction is close to even — neither side is structurally favored.
    #[test]
    fn buy_fraction_is_near_balanced_from_neutral_start() {
        let c = FlowConfig {
            buy_bias_init: 0.5,
            ..cfg()
        };
        let mut flow = Flow::new(c);
        let mut buys = 0u32;
        let mut total = 0u32;
        for _ in 0..40_000 {
            for o in flow.tick() {
                total += 1;
                if o.side == SwapSide::Buy {
                    buys += 1;
                }
            }
        }
        let frac = buys as f64 / total as f64;
        assert!(
            (frac - 0.5).abs() < 0.05,
            "buy fraction {frac} not near 0.5"
        );
    }

    /// The buy-bias stays strictly inside the configured clamp bounds, so the
    /// side draw never degenerates to always-buy or always-sell.
    #[test]
    fn buy_bias_stays_within_bounds() {
        let c = cfg();
        let (min, max) = (c.buy_bias_min, c.buy_bias_max);
        let mut flow = Flow::new(c);
        for _ in 0..10_000 {
            flow.tick();
            let b = flow.buy_bias();
            assert!(b >= min && b <= max, "bias {b} escaped [{min}, {max}]");
        }
    }

    /// Sizes are positive and their median tracks `median_notional` (the
    /// defining property of the `LogNormal(ln(median), σ)` parameterization).
    #[test]
    fn notional_median_tracks_config() {
        let c = cfg();
        let median = c.median_notional;
        let mut flow = Flow::new(c);
        let mut sizes = Vec::new();
        for _ in 0..50_000 {
            for o in flow.tick() {
                assert!(o.notional > 0.0);
                sizes.push(o.notional);
            }
        }
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let sample_median = sizes[sizes.len() / 2];
        let ratio = sample_median / median;
        assert!(
            (0.9..1.1).contains(&ratio),
            "sample median {sample_median} far from configured {median}",
        );
    }

    /// A zero shock with full reversion collapses the bias to exactly 0.5 and
    /// holds it there — the deterministic, shock-free limit.
    #[test]
    fn zero_shock_collapses_bias_to_half() {
        let c = FlowConfig {
            buy_bias_init: 0.8,
            buy_bias_reversion: 1.0,
            buy_bias_shock: 0.0,
            ..cfg()
        };
        let mut flow = Flow::new(c);
        // One order steps the bias once; full reversion lands it on 0.5.
        while flow.tick().is_empty() {}
        assert!((flow.buy_bias() - 0.5).abs() < 1e-12);
    }
}
