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
//! On top of the per-order coin sits an **inventory skew**: the flow tracks
//! its cumulative net take (quote notional bought minus sold, fed back by the
//! caller through [`Flow::record_take`]) and shifts the coin toward the
//! correcting side in logit space, `P(buy) = σ(logit(buy_bias) − k · net)`.
//! The `buy_bias` walk alone is unbiased, so its cumulative net grows without
//! bound and a long demo eats one side of the book; the skew makes net
//! mean-revert while every side is still a random draw. `k = 0` recovers the
//! pure walk.
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
    /// Current `P(buy)` for the next order, in `(0, 1)`, before the
    /// inventory skew.
    buy_bias: f64,
    /// Cumulative net quote notional taken: bought minus sold, as recorded
    /// through [`Flow::record_take`].
    net_taken: f64,
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
            net_taken: 0.0,
        }
    }

    /// The cumulative net quote notional taken (for logging / inspection).
    pub fn net_taken(&self) -> f64 {
        self.net_taken
    }

    /// Record a take that actually executed, as its quote notional, so the
    /// inventory skew acts on what traded rather than on the raw draws (a live
    /// take may be depth-capped or skipped after it is sampled).
    pub fn record_take(&mut self, side: SwapSide, quote_notional: f64) {
        match side {
            SwapSide::Buy => self.net_taken += quote_notional,
            SwapSide::Sell => self.net_taken -= quote_notional,
        }
    }

    /// The `P(buy)` the next side draw uses: the walk's `buy_bias` shifted in
    /// logit space by `-inventory_skew · net_taken`, then clamped to the
    /// configured bounds (which double as the skew's cap). A zero skew
    /// returns `buy_bias` exactly.
    pub fn effective_buy_prob(&self) -> f64 {
        let k = self.cfg.inventory_skew;
        if k == 0.0 {
            return self.buy_bias;
        }
        let logit = (self.buy_bias / (1.0 - self.buy_bias)).ln();
        let p = 1.0 / (1.0 + (k * self.net_taken - logit).exp());
        p.clamp(self.cfg.buy_bias_min, self.cfg.buy_bias_max)
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

    /// Sample one order: its side from the inventory-skewed buy-bias, then
    /// nudge the bias back toward 0.5 with a Gaussian shock (so flow is
    /// autocorrelated but mean-reverting), and its notional from the
    /// LogNormal size law.
    fn next_order(&mut self) -> Order {
        let side = if self.rng.gen::<f64>() < self.effective_buy_prob() {
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

    /// Run `ticks` ticks, recording every draw as executed (the dry-run
    /// model), and return the largest |net take| seen and the order count.
    fn max_abs_net(c: FlowConfig, ticks: u32) -> (f64, u32) {
        let mut flow = Flow::new(c);
        let (mut max, mut orders) = (0.0f64, 0u32);
        for _ in 0..ticks {
            for o in flow.tick() {
                orders += 1;
                flow.record_take(o.side, o.notional);
                max = max.max(flow.net_taken().abs());
            }
        }
        (max, orders)
    }

    /// Over ~50k orders (about eleven hours of default demo flow) the default
    /// skew holds the *peak* |net take| under one side of the ~$100 seeded
    /// book, where the pure walk (`k = 0`, same seed) wanders into the
    /// thousands. Measured peaks sit near $60 for any `k` in 0.05..=0.1 and
    /// rise past it, the clamp and the size tail then setting the peak.
    #[test]
    fn inventory_skew_bounds_net_take() {
        let ticks = 20_000;
        let (skewed, orders) = max_abs_net(cfg(), ticks);
        let (walk, _) = max_abs_net(
            FlowConfig {
                inventory_skew: 0.0,
                ..cfg()
            },
            ticks,
        );
        assert!(orders > 10_000, "only {orders} orders sampled");
        assert!(skewed < 80.0, "skewed max |net| {skewed} too large");
        assert!(
            walk > 10.0 * skewed,
            "pure walk {walk} should wander far past skewed {skewed}",
        );
    }

    /// A zero skew leaves the side draw on the walk's own bias whatever the
    /// net, so `k = 0` is exactly today's pure random walk.
    #[test]
    fn zero_skew_ignores_net() {
        let mut flow = Flow::new(FlowConfig {
            inventory_skew: 0.0,
            ..cfg()
        });
        flow.record_take(SwapSide::Buy, 1_000.0);
        assert_eq!(flow.effective_buy_prob(), flow.buy_bias());
    }

    /// Net buying pushes `P(buy)` below the walk's bias and net selling above
    /// it, never past the clamp bounds.
    #[test]
    fn skew_favors_the_correcting_side_within_bounds() {
        let c = cfg();
        let (min, max) = (c.buy_bias_min, c.buy_bias_max);
        let mut flow = Flow::new(c);
        let base = flow.buy_bias();
        flow.record_take(SwapSide::Buy, 20.0);
        let after_buys = flow.effective_buy_prob();
        assert!(after_buys < base, "{after_buys} should be under {base}");
        flow.record_take(SwapSide::Sell, 40.0);
        let after_sells = flow.effective_buy_prob();
        assert!(after_sells > base, "{after_sells} should exceed {base}");
        flow.record_take(SwapSide::Buy, 1e9);
        assert_eq!(flow.effective_buy_prob(), min);
        flow.record_take(SwapSide::Sell, 2e9);
        assert_eq!(flow.effective_buy_prob(), max);
    }

    /// The skewed flow stays a coin, not a schedule: under the default skew
    /// the side still changes on a large share of consecutive orders and
    /// repeats on a large share too.
    #[test]
    fn skewed_flow_stays_stochastic() {
        let mut flow = Flow::new(cfg());
        let (mut prev, mut switches, mut pairs) = (None, 0u32, 0u32);
        for _ in 0..20_000 {
            for o in flow.tick() {
                flow.record_take(o.side, o.notional);
                if let Some(p) = prev {
                    pairs += 1;
                    if p != o.side {
                        switches += 1;
                    }
                }
                prev = Some(o.side);
            }
        }
        let frac = switches as f64 / pairs as f64;
        assert!(
            (0.3..0.7).contains(&frac),
            "side switched on {frac} of consecutive orders",
        );
    }
}
