//! Set-and-forget passive liquidity — post a flipping ladder on the vault and
//! leave it, re-centering by hand when the market has walked.
//!
//! The operating model is a laptop visit about once a day: open the panel,
//! post or re-center, sign, close it. Nothing stays connected in between, so
//! there is no maker process here — the book rests and flips on its own.
//!
//! The same two halves as [`crate::leader`] around the operator's typed `yes`:
//! [`prepare`] reads the vault fresh and builds a [`Ticket`] from the
//! [`Form`]; the panel previews it; [`execute`] sends exactly that ticket. The
//! ladder comes from [`dropset_sdk::ladder`]: by default its passive-tight-8
//! [`TightLadder`], the builder the maker bot's passive mode uses, so the two
//! cannot drift on the shape rules; an evenly spaced [`PassiveLadder`] is the
//! alternative.
//!
//! Both writes are the vault's quote authority's to sign, so the vault is
//! found by that key and the leader — the custody key — is never needed: a
//! daily visit loads only the quote key.
//!
//! A blank center is the estimator's fair when a fair-price database is
//! configured ([`crate::fair`]), and the vault's live reference or the book
//! mid when none is. The form opens over [`vault_lines`] — where the fair
//! sits against the stamped center, then what rested and what flipped since
//! the last stamp — so the daily visit sees the drift and the fills before it
//! re-centers.
//!
//! The stop is not here: the emergency kill (`K` on mainnet) zeroes every
//! level on the quote key's vaults, passive ones included.

use crate::accounts::{self, MarketView};
use crate::book;
use crate::chain;
use crate::cluster::Cluster;
use crate::emergency::{self, SectorSeat};
use crate::fair::{Fair, Spot, SPOT_SOURCE};
use crate::job::Logger;
use crate::market;
use anyhow::{anyhow, bail, Context, Result};
use dropset_sdk::clock::{SlotTime, WallSpan, WallTime};
use dropset_sdk::ladder::{
    build_profile, passive_shape_warnings, LadderLevel, PassiveLadder, PassiveShape, TightLadder,
    MAX_TIP_BPS,
};
use dropset_sdk::layout::{Level, MarketView as SlabView, Position, Vault, N_LEVELS};
use dropset_sdk::price::Price;
use dropset_sdk::quoting::{profile_bytes, set_liquidity_profile_ix, set_reference_price_ix};
use dropset_util::decimals::human_to_atoms_ratio;
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;

/// What the prompt sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveOp {
    /// Write the ladder and stamp the reference — one transaction.
    Deploy,
    /// Stamp a new reference under the resting ladder — the whole book moves.
    Recenter,
}

impl PassiveOp {
    pub fn label(self) -> &'static str {
        match self {
            PassiveOp::Deploy => "Passive ladder",
            PassiveOp::Recenter => "Passive re-center",
        }
    }
}

/// One editable line of the [`Form`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Price,
    Shape,
    Horizon,
    Tip,
    FlipFraction,
    Spacing,
    Levels,
    FlipSpread,
    Expiry,
}

/// Which builder a deploy posts through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Book {
    /// [`TightLadder`], the default: a tip, then a bell over geometric offsets.
    Tight,
    /// [`PassiveLadder`] in its bell or uniform shape, at one spacing.
    Spaced,
}

/// The shape line's three choices, in the order `→` steps through them.
const SHAPES: [(Book, PassiveShape); 3] = [
    (Book::Tight, PassiveShape::Bell),
    (Book::Spaced, PassiveShape::Bell),
    (Book::Spaced, PassiveShape::Uniform),
];

// Both deploy lists keep the shape on line 1, so switching book leaves the
// cursor where it was.
const TIGHT_FIELDS: [Field; 6] = [
    Field::Price,
    Field::Shape,
    Field::Horizon,
    Field::Tip,
    Field::FlipFraction,
    Field::Expiry,
];
const SPACED_FIELDS: [Field; 6] = [
    Field::Price,
    Field::Shape,
    Field::Spacing,
    Field::Levels,
    Field::FlipSpread,
    Field::Expiry,
];
const RECENTER_FIELDS: [Field; 1] = [Field::Price];

/// The visit horizon steps by a day, up to a week between visits.
const MAX_HORIZON_DAYS: u32 = 7;
/// The tip steps by half a percent of the leg, up to the builder's cap.
const TIP_STEP_BPS: u16 = 50;
/// The flip fraction steps by a tenth of the gap, from a tenth to the whole.
const FLIP_FRACTION_STEP_PCT: u32 = 10;
const MAX_FLIP_FRACTION_PCT: u32 = 100;

/// The spacing steps by 1 bp, between 1 and 100 bps — eight rungs at the
/// widest still sit within 8% of the center.
const SPACING_STEP_PPM: u32 = 100;
const MAX_SPACING_PPM: u32 = 10_000;
/// The flip spread steps by half a bp; `0` is allowed and warned. Capped at
/// the widest spacing, past which every rung would warn anyway.
const FLIP_STEP_PPM: u32 = 50;
const MAX_FLIP_SPREAD_PPM: u32 = MAX_SPACING_PPM;

/// How far a typed center may sit from the vault's live reference before the
/// confirmation warns — a fat-fingered digit, not a day's FX move.
const CENTER_DEVIATION_WARN: f64 = 0.05;

/// The level lives the expiry field steps through — unbounded first, since a
/// passive book is meant to be walked away from, then horizons that bound a
/// missed visit.
pub const EXPIRY_PRESETS: [WallSpan; 5] = [
    WallSpan::UNBOUNDED,
    WallSpan::new(6 * 3_600),
    WallSpan::new(24 * 3_600),
    WallSpan::new(48 * 3_600),
    WallSpan::new(7 * 24 * 3_600),
];

/// The longest typed price — room for a quote with its decimals, not a paste.
const MAX_PRICE_CHARS: usize = 16;

/// The open form: the ladder's parameters and the price to center it on.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub op: PassiveOp,
    /// Which of the two ladders below a deploy posts.
    pub book: Book,
    pub tight: TightLadder,
    pub ladder: PassiveLadder,
    /// Typed human quote-per-base; blank takes the estimator's fair, or —
    /// with no fair-price database — re-stamps the vault's live reference,
    /// or centers on the book mid when it has none.
    pub price: String,
    /// Index into [`Form::fields`] of the line being edited.
    pub cursor: usize,
    /// The fair as the form opened — what a blank center reads as. The
    /// ticket reads it again, fresh.
    pub fair: Fair,
    /// [`vault_lines`] as the form opened, or the reason it could not read.
    pub vault: Vec<String>,
}

impl Form {
    pub fn new(op: PassiveOp) -> Self {
        Self {
            op,
            book: Book::Tight,
            tight: TightLadder::default(),
            ladder: PassiveLadder::default(),
            price: String::new(),
            cursor: 0,
            fair: Fair::Off,
            vault: Vec::new(),
        }
    }

    pub fn fields(&self) -> &'static [Field] {
        match (self.op, self.book) {
            (PassiveOp::Deploy, Book::Tight) => &TIGHT_FIELDS,
            (PassiveOp::Deploy, Book::Spaced) => &SPACED_FIELDS,
            (PassiveOp::Recenter, _) => &RECENTER_FIELDS,
        }
    }

    /// The rungs a deploy posts.
    pub fn levels(&self) -> Vec<LadderLevel> {
        match self.book {
            Book::Tight => self.tight.levels(),
            Book::Spaced => self.ladder.levels(),
        }
    }

    /// Whether [`Self::levels`]' L1 is a declared tip.
    pub fn has_tip(&self) -> bool {
        self.book == Book::Tight
    }

    fn shape_at(&self) -> usize {
        let shape = match self.book {
            Book::Tight => PassiveShape::Bell,
            Book::Spaced => self.ladder.shape,
        };
        SHAPES
            .iter()
            .position(|&s| s == (self.book, shape))
            .unwrap_or(0)
    }

    fn expiry_mut(&mut self) -> &mut WallSpan {
        match self.book {
            Book::Tight => &mut self.tight.expiry_secs,
            Book::Spaced => &mut self.ladder.expiry_secs,
        }
    }

    pub fn field(&self) -> Field {
        self.fields()[self.cursor]
    }

    /// Move the cursor by `delta` lines, clamped.
    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.fields().len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
    }

    /// Step the current field by `delta` (`←` is `-1`, `→` is `+1`).
    pub fn step(&mut self, delta: i32) {
        let field = self.field();
        let up = delta > 0;
        let at = self.shape_at();
        let t = &mut self.tight;
        let l = &mut self.ladder;
        match field {
            Field::Price => {}
            Field::Shape => {
                let next = if up {
                    (at + 1) % SHAPES.len()
                } else {
                    (at + SHAPES.len() - 1) % SHAPES.len()
                };
                let (book, shape) = SHAPES[next];
                self.book = book;
                if book == Book::Spaced {
                    l.shape = shape;
                }
            }
            Field::Horizon => {
                t.horizon_days = stepped(t.horizon_days, 1, up).clamp(1, MAX_HORIZON_DAYS);
            }
            Field::Tip => {
                t.tip_size_bps = if up {
                    t.tip_size_bps.saturating_add(TIP_STEP_BPS)
                } else {
                    t.tip_size_bps.saturating_sub(TIP_STEP_BPS)
                }
                .clamp(TIP_STEP_BPS, MAX_TIP_BPS);
            }
            Field::FlipFraction => {
                t.flip_fraction_pct = stepped(t.flip_fraction_pct, FLIP_FRACTION_STEP_PCT, up)
                    .clamp(FLIP_FRACTION_STEP_PCT, MAX_FLIP_FRACTION_PCT);
            }
            Field::Spacing => {
                l.spacing_ppm = stepped(l.spacing_ppm, SPACING_STEP_PPM, up)
                    .clamp(SPACING_STEP_PPM, MAX_SPACING_PPM);
            }
            Field::Levels => {
                l.levels = if up {
                    l.levels + 1
                } else {
                    l.levels.saturating_sub(1)
                }
                .clamp(1, N_LEVELS);
            }
            Field::FlipSpread => {
                l.flip_spread_ppm =
                    stepped(l.flip_spread_ppm, FLIP_STEP_PPM, up).min(MAX_FLIP_SPREAD_PPM);
            }
            Field::Expiry => {
                let expiry = self.expiry_mut();
                let at = EXPIRY_PRESETS.iter().position(|e| e == expiry).unwrap_or(0);
                let next = if up {
                    (at + 1).min(EXPIRY_PRESETS.len() - 1)
                } else {
                    at.saturating_sub(1)
                };
                *expiry = EXPIRY_PRESETS[next];
            }
        }
    }

    /// Type `c` into the price — the one free-text field. Anything else is
    /// ignored rather than swallowed into a number it would corrupt.
    pub fn type_char(&mut self, c: char) {
        if self.field() == Field::Price
            && (c.is_ascii_digit() || c == '.')
            && self.price.len() < MAX_PRICE_CHARS
        {
            self.price.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.field() == Field::Price {
            self.price.pop();
        }
    }

    /// The form's lines, the edited one marked — what the prompt draws.
    pub fn lines(&self) -> Vec<String> {
        let l = &self.ladder;
        let t = &self.tight;
        let reach = |levels: Vec<LadderLevel>| match (levels.first(), levels.last()) {
            (Some(inner), Some(outer)) => {
                format!("{} to {}", bps(inner.offset_ppm), bps(outer.offset_ppm))
            }
            _ => String::new(),
        };
        self.fields()
            .iter()
            .enumerate()
            .map(|(i, field)| {
                let value = match field {
                    Field::Price if self.price.is_empty() => match &self.fair {
                        Fair::Off | Fair::Unpublished => "live reference (or book mid)".to_string(),
                        Fair::Live { .. } => self.fair.describe(),
                        _ => format!("type one — {}", self.fair.describe()),
                    },
                    Field::Price => self.price.clone(),
                    Field::Shape => match (self.book, l.shape) {
                        (Book::Tight, _) => "tight-8 (a tip, then a bell)".to_string(),
                        (Book::Spaced, PassiveShape::Bell) => "bell, evenly spaced".to_string(),
                        (Book::Spaced, PassiveShape::Uniform) => {
                            "uniform, evenly spaced".to_string()
                        }
                    },
                    Field::Horizon => {
                        let days = if t.horizon_days == 1 { "day" } else { "days" };
                        format!("{} {days} — {}", t.horizon_days, reach(t.levels()))
                    }
                    Field::Tip => format!("{:.1}% of the leg at L1", t.tip_size_bps as f64 / 100.0),
                    Field::FlipFraction => format!("{}% of each rung's gap", t.flip_fraction_pct),
                    Field::Spacing => bps(l.spacing_ppm),
                    Field::Levels => format!("{} a side", l.levels),
                    Field::FlipSpread => bps(l.flip_spread_ppm),
                    Field::Expiry => expiry_label(match self.book {
                        Book::Tight => t.expiry_secs,
                        Book::Spaced => l.expiry_secs,
                    }),
                };
                let name = match field {
                    Field::Price => "center price",
                    Field::Shape => "shape",
                    Field::Horizon => "visit horizon",
                    Field::Tip => "tip size",
                    Field::FlipFraction => "flip fraction",
                    Field::Spacing => "spacing",
                    Field::Levels => "levels",
                    Field::FlipSpread => "flip spread",
                    Field::Expiry => "expiry",
                };
                let mark = if i == self.cursor { '\u{25b8}' } else { ' ' };
                format!("{mark} {name:<13} {value}")
            })
            .collect()
    }
}

fn stepped(value: u32, step: u32, up: bool) -> u32 {
    if up {
        value.saturating_add(step)
    } else {
        value.saturating_sub(step)
    }
}

fn bps(ppm: u32) -> String {
    format!("{:.1} bps", ppm as f64 / 100.0)
}

fn expiry_label(span: WallSpan) -> String {
    if span == WallSpan::UNBOUNDED {
        "never (until a re-center or kill)".to_string()
    } else {
        format!("{} h", span.get() / 3_600)
    }
}

/// A positive, finite typed price, or `None`.
pub fn parse_price(buf: &str) -> Option<f64> {
    buf.parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && *p > 0.0)
}

/// A built, not-yet-sent passive write on one vault.
#[derive(Clone, Debug)]
pub struct Ticket {
    pub op: PassiveOp,
    pub market: Pubkey,
    /// Resolves the pair, and through it the signing quote authority.
    pub base_mint: Pubkey,
    pub base_symbol: &'static str,
    pub quote_symbol: &'static str,
    pub seat: SectorSeat,
    /// The vault's quote authority — the key both writes are signed by.
    pub quote_authority: Pubkey,
    /// The rungs [`PassiveOp::Deploy`] writes; empty on a re-center.
    pub levels: Vec<LadderLevel>,
    /// The center, human quote-per-base, and where it came from.
    pub center: f64,
    pub center_source: &'static str,
    pub price: Price,
    pub warnings: Vec<String>,
}

impl Ticket {
    /// The confirmation's body — what the operator says `yes` to.
    pub fn summary(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "market  {}/{}  vault #{}",
                self.base_symbol, self.quote_symbol, self.seat.seq
            ),
            format!("signer  {} (quote authority)", self.quote_authority),
            format!(
                "center  {} ({})",
                book::fmt_price(self.center),
                self.center_source
            ),
        ];
        match self.op {
            PassiveOp::Deploy => {
                // Every rung shares one life, so the innermost names it.
                if let Some(first) = self.levels.first() {
                    lines.push(format!(
                        "expiry  {}",
                        expiry_label(first.expiry_offset_secs)
                    ));
                }
                lines.extend(ladder_lines(&self.levels, self.center));
            }
            PassiveOp::Recenter => lines.push(
                "the resting ladder moves with it, shape unchanged; bounded expiries restart"
                    .to_string(),
            ),
        }
        lines.extend(self.warnings.iter().map(|w| format!("warning: {w}")));
        lines.push("stop: the emergency kill (K on mainnet) zeroes it".to_string());
        lines
    }
}

/// Width of the widest size bar in the preview.
const BAR_WIDTH: usize = 20;

/// The preview: one line per rung, outermost ask first down to outermost bid,
/// the way the book pane reads, each with its price and a size bar.
pub fn ladder_lines(levels: &[LadderLevel], center: f64) -> Vec<String> {
    let widest = levels.iter().map(|l| l.size_bps).max().unwrap_or(1).max(1);
    let line = |side: &str, i: usize, lvl: &LadderLevel, sign: f64| {
        let price = center * (1.0 + sign * lvl.offset_ppm as f64 / 1e6);
        let bar = "\u{2588}".repeat(usize::from(lvl.size_bps) * BAR_WIDTH / usize::from(widest));
        format!(
            "{side} L{}  {:>12}  {:>5.1}%  flip {:>4.1} bp  {bar}",
            i + 1,
            book::fmt_price(price),
            lvl.size_bps as f64 / 100.0,
            lvl.flip_spread_ppm as f64 / 100.0
        )
    };
    let mut lines: Vec<String> = levels
        .iter()
        .enumerate()
        .rev()
        .map(|(i, l)| line("ask", i, l, 1.0))
        .collect();
    lines.push(format!("    ----  {:>12}  center", book::fmt_price(center)));
    lines.extend(
        levels
            .iter()
            .enumerate()
            .map(|(i, l)| line("bid", i, l, -1.0)),
    );
    lines
}

/// The one live vault on `market` whose quote authority is `quote_authority`,
/// read fresh, that a passive write can sign for — refusing none, several, or
/// a frozen one.
fn passive_seat(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    quote_authority: &Pubkey,
) -> Result<SectorSeat> {
    let seats = emergency::quote_seats(client, cluster, market, quote_authority)?
        .context("market not found")?
        .seats;
    pick_seat(seats, quote_authority)
}

/// [`passive_seat`]'s choice over one read's `seats` — split out so the
/// refusals are testable without a chain.
fn pick_seat(seats: Vec<SectorSeat>, quote_authority: &Pubkey) -> Result<SectorSeat> {
    let live: Vec<_> = seats.into_iter().filter(|s| !s.tombstoned).collect();
    let seat = match live[..] {
        [seat] => seat,
        [] => bail!("quote authority {quote_authority} signs for no live vault on this market"),
        ref many => bail!(
            "quote authority {quote_authority} signs for {} vaults on this market (#{}) — \
             refusing to guess",
            many.len(),
            many.iter()
                .map(|s| s.seq.to_string())
                .collect::<Vec<_>>()
                .join(", #")
        ),
    };
    if seat.frozen {
        bail!("vault #{} is frozen — matching is stopped", seat.seq);
    }
    // The scan keys on this field, so it holds by construction; kept as the
    // guard, since the program checks exactly this key.
    if seat.quote_authority != *quote_authority {
        bail!(
            "vault #{}'s quote authority is not this session's quote key — it cannot sign",
            seat.seq
        );
    }
    Ok(seat)
}

/// Read the vault `quote_authority` signs for on `market` fresh and build the
/// ticket `form` describes, a blank center taking `fair` — read fresh by the
/// caller — when it governs. Sends nothing.
pub fn prepare(
    client: &RpcClient,
    repo_root: &Path,
    cluster: Cluster,
    market: &MarketView,
    quote_authority: &Pubkey,
    form: &Form,
    fair: &Fair,
) -> Result<Ticket> {
    let config = market::config_for(repo_root, cluster, &market.base_mint)
        .context("the selected market is not in this cluster's roster")?;
    let seat = passive_seat(client, cluster, &market.address, quote_authority)?;
    if form.op == PassiveOp::Recenter && !seat.quoting {
        bail!("no ladder rests on vault #{} — post one first", seat.seq);
    }
    let reference = accounts::read_reference_price(client, &market.address, seat.idx)
        .context("read the vault's reference price")?;
    let live = reference
        .is_matchable()
        .then(|| book::human_price(reference, market.base_decimals, market.quote_decimals));
    // Only a two-sided book has a mid; one side alone is not a center.
    let mid = (!market.asks.is_empty() && !market.bids.is_empty())
        .then(|| book::mid_price(market))
        .flatten();
    let (center, center_source) = resolve_center(&form.price, live, mid, fair)?;
    let ratio = human_to_atoms_ratio(center, market.base_decimals, market.quote_decimals);
    let price = Price::from_value(ratio).with_context(|| format!("encode the center {center}"))?;
    let (levels, mut warnings) = match form.op {
        PassiveOp::Deploy => {
            let levels = form.levels();
            let warnings = passive_shape_warnings(&levels, form.has_tip());
            (levels, warnings)
        }
        PassiveOp::Recenter => (Vec::new(), Vec::new()),
    };
    warnings.extend(live.and_then(|live| center_warning(center, live, "live reference")));
    // Silent when the center is the fair itself.
    warnings.extend(
        fair.price()
            .and_then(|f| center_warning(center, f, "estimator's fair")),
    );
    Ok(Ticket {
        op: form.op,
        market: market.address,
        base_mint: market.base_mint,
        base_symbol: config.base.symbol,
        quote_symbol: config.quote.symbol,
        seat,
        quote_authority: *quote_authority,
        levels,
        center,
        center_source,
        price,
        warnings,
    })
}

/// The center `typed` names, or — blank — the one the session's sources give,
/// with where it came from. Split out of [`prepare`] so the precedence is
/// testable without a chain: a typed price wins; with an estimator to ask, a
/// blank center is its fair and nothing older, since a paused or unreadable
/// one is a reason to stop, not to keep quoting yesterday's center; without
/// one, the vault's `live` reference, then the two-sided book's `mid`.
fn resolve_center(
    typed: &str,
    live: Option<f64>,
    mid: Option<f64>,
    fair: &Fair,
) -> Result<(f64, &'static str)> {
    if let Some(typed) = parse_price(typed) {
        return Ok((typed, "typed"));
    }
    if !typed.is_empty() {
        bail!("enter a positive number, e.g. 1.324");
    }
    if fair.governs() {
        return match fair.price() {
            Some(price) => Ok((price, "the estimator's fair")),
            None => bail!("{} — type a price", fair.describe()),
        };
    }
    match (live, mid) {
        // A blank re-stamp is the daily keep-alive: same center, fresh stamp.
        (Some(live), _) => Ok((live, "the vault's live reference, re-stamped")),
        (None, Some(mid)) => Ok((mid, "the book mid")),
        (None, None) => {
            bail!("the vault has no reference and the book is not two-sided — type a price")
        }
    }
}

/// A warning when `center` sits more than [`CENTER_DEVIATION_WARN`] from
/// `against` — the vault's live reference or the estimator's fair, named by
/// `what` — the one check between a mistyped digit and a book that is swept
/// on arrival.
fn center_warning(center: f64, against: f64, what: &str) -> Option<String> {
    let off = center / against - 1.0;
    (off.abs() > CENTER_DEVIATION_WARN).then(|| {
        format!(
            "the center is {:+.1}% from the {what} {} — check the digits",
            off * 100.0,
            book::fmt_price(against)
        )
    })
}

/// The per-vault view the form opens over: the quote key's vault on `market`,
/// read fresh — its drift from `fair` (and `spot`, when stored; see
/// `drift_lines`), then level by level (see `ladder_state`). Its symbols
/// name the legs.
pub fn vault_lines(
    client: &RpcClient,
    repo_root: &Path,
    cluster: Cluster,
    market: &MarketView,
    quote_authority: &Pubkey,
    fair: &Fair,
    spot: Option<Spot>,
) -> Result<Vec<String>> {
    let config = market::config_for(repo_root, cluster, &market.base_mint)
        .context("the selected market is not in this cluster's roster")?;
    let seat = passive_seat(client, cluster, &market.address, quote_authority)?;
    let account =
        chain::fetch_fresh(client, &market.address, "market")?.context("market not found")?;
    let view = SlabView::load(&account.data)
        .map_err(|_| anyhow!("{} does not decode as a market slab", market.address))?;
    let vault = view
        .sectors()
        .get(seat.idx as usize)
        .filter(|v| v.seq.get() == seat.seq)
        .with_context(|| format!("vault #{} moved between two reads — reopen", seat.seq))?;
    let legs = Legs {
        base: Leg {
            symbol: config.base.symbol,
            decimals: market.base_decimals,
        },
        quote: Leg {
            symbol: config.quote.symbol,
            decimals: market.quote_decimals,
        },
    };
    // Expiry is dual-domain, so the view needs both clocks.
    let slot = client.get_slot().context("current slot")?;
    let now = (
        dropset_sdk::time::now_unix_u32(),
        SlotTime::new(slot.min(u64::from(u32::MAX)) as u32),
    );
    let mut lines = ladder_state(vault, seat.seq, &legs, now);
    // Under the stamp line, where the operator reads the center from.
    lines.splice(1..1, drift_lines(vault, &legs, fair, spot));
    Ok(lines)
}

/// Where the market has walked since the stamp, as the daily visit decides a
/// re-center: the fair beside the center the book rests on, the gap in bps
/// and in the rungs it has walked past on its side, the [`Spot`] beside the
/// fair when one is stored, and either side with nothing left resting.
/// Empty for a vault posting no ladder — `ladder_state` says so.
fn drift_lines(vault: &Vault, legs: &Legs, fair: &Fair, spot: Option<Spot>) -> Vec<String> {
    let profile = &vault.profile;
    if profile.size_bps_sums() == (0, 0) {
        return Vec::new();
    }
    let rp = &vault.reference_price;
    let mut lines = Vec::new();
    let center = Price::from_bits(rp.price.get())
        .is_matchable()
        .then(|| legs.price(rp.price.get()));
    let used = |side: &[Level; N_LEVELS]| -> Vec<u32> {
        side.iter()
            .filter(|l| l.size_bps.get() > 0)
            .map(|l| l.price_offset_ppm.get())
            .collect()
    };
    match (center, fair.price()) {
        (None, _) => lines.push("center  none stamped — the book is dark".to_string()),
        (Some(center), Some(fair)) => {
            let gap_bps = (fair / center - 1.0) * 1e4;
            let side = if gap_bps > 0.0 {
                used(&profile.asks)
            } else {
                used(&profile.bids)
            };
            let walked = side
                .iter()
                .filter(|&&offset| f64::from(offset) <= gap_bps.abs() * 100.0)
                .count();
            lines.push(format!(
                "fair {} · center {} · gap {gap_bps:+.1} bps, past {walked} of {} rungs",
                book::fmt_price(fair),
                book::fmt_price(center),
                side.len()
            ));
        }
        (Some(center), None) => lines.push(format!(
            "center {} · {}",
            book::fmt_price(center),
            fair.describe()
        )),
    }
    if let Some(spot) = spot {
        let against = fair
            .price()
            .map(|f| format!("{:+.1} bps vs fair, ", (spot.price / f - 1.0) * 1e4))
            .unwrap_or_default();
        lines.push(format!(
            "{SPOT_SOURCE} spot {} ({against}{} s old)",
            book::fmt_price(spot.price),
            spot.age_secs
        ));
    }
    // A stamp's flush is still pending, so every posted rung rests as posted.
    let armed = rp.flush_armed();
    let rests = |sizes: &[Level; N_LEVELS], left: &[Position; N_LEVELS]| {
        (0..N_LEVELS).any(|i| sizes[i].size_bps.get() > 0 && (armed || left[i].size.get() > 0))
    };
    for (side, sizes, left) in [
        ("ask", &profile.asks, &vault.remaining.asks),
        ("bid", &profile.bids, &vault.remaining.bids),
    ] {
        if !rests(sizes, left) {
            lines.push(format!(
                "no {side} rests — the book is one-sided until a re-center"
            ));
        }
    }
    lines
}

/// One leg's symbol and mint decimals.
#[derive(Clone, Copy)]
struct Leg {
    symbol: &'static str,
    decimals: u8,
}

impl Leg {
    /// `atoms` of this leg in whole units, with its symbol.
    fn amount(self, atoms: u64) -> String {
        let units = atoms as f64 / 10f64.powi(i32::from(self.decimals));
        format!("{units:.2} {}", self.symbol)
    }
}

/// The market's two legs.
struct Legs {
    base: Leg,
    quote: Leg,
}

impl Legs {
    /// A stored price's bits in human quote-per-base.
    fn price(&self, bits: u32) -> f64 {
        book::human_price(
            Price::from_bits(bits),
            self.base.decimals,
            self.quote.decimals,
        )
    }
}

/// What `vault` rests and what has flipped since its last stamp, outermost
/// ask down to outermost bid. A stamp zeroes every flipped counter, so for a
/// visit that ends on a re-center or re-stamp, "since the stamp" is "since
/// the last visit". Until the first taker after a stamp nothing has traded,
/// and the profile rests exactly as posted.
///
/// A level is dead once either of its deadlines is at or past `now` — the
/// matcher rests one only inside both.
fn ladder_state(
    vault: &Vault,
    seq: u64,
    legs: &Legs,
    (now_wall, now_slot): (WallTime, SlotTime),
) -> Vec<String> {
    let rp = &vault.reference_price;
    let stamped = rp.quote_unix.get();
    let hours = f64::from(now_wall.get().saturating_sub(stamped)) / 3_600.0;
    let mut lines = vec![format!(
        "vault #{seq}  stamped {hours:.1} h ago at {}",
        book::fmt_price(legs.price(rp.price.get()))
    )];
    let profile = &vault.profile;
    if profile.size_bps_sums() == (0, 0) {
        lines.push("no ladder rests on this vault".to_string());
        return lines;
    }
    let dead = |(wall, slot): (WallTime, SlotTime)| wall <= now_wall || slot <= now_slot;
    let used = |sizes: &[Level; N_LEVELS], i: usize| sizes[i].size_bps.get() > 0;
    if rp.flush_armed() {
        // Every rung of a posted ladder shares one life, so the first used
        // one — on either side — names it.
        let first = (0..N_LEVELS)
            .find_map(|i| used(&profile.asks, i).then_some(&profile.asks[i]))
            .or_else(|| {
                (0..N_LEVELS).find_map(|i| used(&profile.bids, i).then_some(&profile.bids[i]))
            })
            .expect("a nonzero side sum has a used level");
        lines.push(if dead(first.deadlines(rp)) {
            "nothing traded since the stamp, and the ladder has expired".to_string()
        } else {
            "nothing traded since the stamp — every level rests as posted".to_string()
        });
        return lines;
    }
    let mut flipped = 0;
    let mut level = |side: &str, i: usize, p: &Position, own: Leg, other: Leg| {
        let expired = if dead((p.wall_deadline(), p.slot_deadline())) {
            "  expired"
        } else {
            ""
        };
        let flip = if p.flip_size.get() == 0 {
            String::new()
        } else {
            flipped += 1;
            format!(
                "  flipped {} @ {}",
                other.amount(p.flip_size.get()),
                book::fmt_price(legs.price(p.flip_price.get()))
            )
        };
        format!(
            "{side} L{}  {:>12}  rests {}{flip}{expired}",
            i + 1,
            book::fmt_price(legs.price(p.price.get())),
            own.amount(p.size.get()),
        )
    };
    let mut rungs: Vec<String> = (0..N_LEVELS)
        .rev()
        .filter(|&i| used(&profile.asks, i))
        .map(|i| level("ask", i, &vault.remaining.asks[i], legs.base, legs.quote))
        .collect();
    rungs.extend(
        (0..N_LEVELS)
            .filter(|&i| used(&profile.bids, i))
            .map(|i| level("bid", i, &vault.remaining.bids[i], legs.quote, legs.base)),
    );
    lines.push(match flipped {
        0 => "traded since the stamp, but no level has flipped yet".to_string(),
        1 => "1 level flipped since the stamp:".to_string(),
        n => format!("{n} levels flipped since the stamp:"),
    });
    lines.extend(rungs);
    lines
}

/// Send the confirmed `ticket`, signed by the vault's `quote_authority` and
/// paid by `wallet`: the ladder and the reference in one transaction on a
/// deploy, so the book never rests on the new shape at the old center; the
/// reference alone on a re-center. Refuses, nothing sent, when the vault is no
/// longer the one confirmed.
pub fn execute(
    client: &RpcClient,
    wallet: &Keypair,
    quote_authority: &Keypair,
    cluster: Cluster,
    ticket: &Ticket,
    log: &Logger,
) -> Result<String> {
    if quote_authority.pubkey() != ticket.quote_authority {
        bail!("the signing quote authority is not the one confirmed — nothing was sent");
    }
    let now = passive_seat(client, cluster, &ticket.market, &ticket.quote_authority)?;
    if (now.idx, now.seq) != (ticket.seat.idx, ticket.seat.seq) {
        bail!(
            "vault #{} is no longer at sector {} — nothing was sent; re-open the prompt",
            ticket.seat.seq,
            ticket.seat.idx
        );
    }
    let mut ixs = Vec::new();
    if ticket.op == PassiveOp::Deploy {
        let profile = build_profile(&ticket.levels);
        if profile.validate_size_sums().is_err() {
            bail!("the ladder commits more than the whole leg — nothing was sent");
        }
        ixs.push(set_liquidity_profile_ix(
            cluster.deployment(),
            ticket.quote_authority,
            ticket.market,
            ticket.seat.idx,
            profile_bytes(&profile),
        ));
    }
    let slot = client.get_slot().context("current slot")?;
    ixs.push(set_reference_price_ix(
        cluster.deployment(),
        ticket.quote_authority,
        ticket.market,
        ticket.seat.idx,
        ticket.price,
        slot,
        dropset_sdk::time::now_unix(),
    ));
    let label = ticket.op.label();
    chain::send_logged(client, wallet, &[wallet, quote_authority], &ixs, label, log)
        .context(label)?;
    log.accounts_changed();
    Ok(match ticket.op {
        PassiveOp::Deploy => format!(
            "Passive ladder on {} vault #{}: {} levels a side around {}",
            ticket.base_symbol,
            ticket.seat.seq,
            ticket.levels.len(),
            book::fmt_price(ticket.center)
        ),
        PassiveOp::Recenter => format!(
            "Re-centered {} vault #{} on {}",
            ticket.base_symbol,
            ticket.seat.seq,
            book::fmt_price(ticket.center)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn the_deploy_form_starts_on_the_shared_tight_ladder() {
        let form = Form::new(PassiveOp::Deploy);
        assert_eq!(form.book, Book::Tight);
        assert_eq!(form.levels(), TightLadder::default().levels());
        assert!(form.has_tip());
        assert_eq!(form.field(), Field::Price);
        assert_eq!(form.fields(), &TIGHT_FIELDS);
        let lines = form.lines();
        assert!(
            lines[2].ends_with("1 day — 3.0 bps to 100.0 bps"),
            "{lines:?}"
        );
        assert!(lines[3].ends_with("2.0% of the leg at L1"), "{lines:?}");
        assert!(lines[4].ends_with("100% of each rung's gap"), "{lines:?}");
        // A re-center edits the price alone.
        assert_eq!(Form::new(PassiveOp::Recenter).fields(), &[Field::Price]);
    }

    #[test]
    fn the_shape_line_steps_through_tight_bell_and_uniform() {
        let mut form = Form::new(PassiveOp::Deploy);
        form.move_cursor(1);
        assert_eq!(form.field(), Field::Shape);
        form.step(1);
        assert_eq!(
            (form.book, form.ladder.shape),
            (Book::Spaced, PassiveShape::Bell)
        );
        assert_eq!(form.fields(), &SPACED_FIELDS);
        assert_eq!(form.field(), Field::Shape, "the cursor stays on the shape");
        assert_eq!(form.levels(), PassiveLadder::default().levels());
        assert!(!form.has_tip());
        form.step(1);
        assert_eq!(form.ladder.shape, PassiveShape::Uniform);
        form.step(1);
        assert_eq!(form.book, Book::Tight);
        form.step(-1);
        assert_eq!(
            (form.book, form.ladder.shape),
            (Book::Spaced, PassiveShape::Uniform)
        );
    }

    #[test]
    fn stepping_clamps_every_tight_field_to_its_range() {
        let mut form = Form::new(PassiveOp::Deploy);
        form.move_cursor(2);
        assert_eq!(form.field(), Field::Horizon);
        for _ in 0..20 {
            form.step(1);
        }
        assert_eq!(form.tight.horizon_days, MAX_HORIZON_DAYS);
        for _ in 0..20 {
            form.step(-1);
        }
        assert_eq!(form.tight.horizon_days, 1);
        form.step(1);
        form.step(1);
        // Three days widens every rung by sqrt(3).
        assert_eq!(form.levels()[7].offset_ppm, 17_321);
        form.move_cursor(1);
        assert_eq!(form.field(), Field::Tip);
        for _ in 0..50 {
            form.step(1);
        }
        assert_eq!(form.tight.tip_size_bps, MAX_TIP_BPS);
        for _ in 0..50 {
            form.step(-1);
        }
        assert_eq!(form.tight.tip_size_bps, TIP_STEP_BPS);
        form.move_cursor(1);
        assert_eq!(form.field(), Field::FlipFraction);
        for _ in 0..20 {
            form.step(1);
        }
        assert_eq!(form.tight.flip_fraction_pct, MAX_FLIP_FRACTION_PCT);
        for _ in 0..20 {
            form.step(-1);
        }
        assert_eq!(form.tight.flip_fraction_pct, FLIP_FRACTION_STEP_PCT);
        form.move_cursor(1);
        assert_eq!(form.field(), Field::Expiry);
        form.step(1);
        assert_eq!(form.tight.expiry_secs, EXPIRY_PRESETS[1]);
        assert_eq!(form.ladder.expiry_secs, WallSpan::UNBOUNDED);
        // Every setting the form allows posts a sound tight ladder.
        assert!(passive_shape_warnings(&form.levels(), form.has_tip()).is_empty());
    }

    #[test]
    fn stepping_clamps_every_spaced_field_to_its_range() {
        let mut form = Form::new(PassiveOp::Deploy);
        form.move_cursor(1);
        form.step(1);
        form.move_cursor(1);
        assert_eq!(form.field(), Field::Spacing);
        for _ in 0..200 {
            form.step(-1);
        }
        assert_eq!(form.ladder.spacing_ppm, SPACING_STEP_PPM);
        form.move_cursor(1);
        for _ in 0..20 {
            form.step(1);
        }
        assert_eq!(form.ladder.levels, N_LEVELS);
        for _ in 0..20 {
            form.step(-1);
        }
        assert_eq!(form.ladder.levels, 1);
        form.move_cursor(1);
        assert_eq!(form.field(), Field::FlipSpread);
        for _ in 0..10 {
            form.step(-1);
        }
        assert_eq!(form.ladder.flip_spread_ppm, 0);
        for _ in 0..500 {
            form.step(1);
        }
        assert_eq!(form.ladder.flip_spread_ppm, MAX_FLIP_SPREAD_PPM);
        form.move_cursor(1);
        assert_eq!(form.field(), Field::Expiry);
        for _ in 0..10 {
            form.step(1);
        }
        assert_eq!(form.ladder.expiry_secs, *EXPIRY_PRESETS.last().unwrap());
        assert_eq!(form.tight.expiry_secs, WallSpan::UNBOUNDED);
        // The cursor clamps at the last line.
        form.move_cursor(5);
        assert_eq!(form.field(), Field::Expiry);
    }

    #[test]
    fn only_the_price_field_takes_typed_text() {
        let mut form = Form::new(PassiveOp::Deploy);
        for c in "1.32x4".chars() {
            form.type_char(c);
        }
        assert_eq!(form.price, "1.324");
        form.move_cursor(1);
        form.type_char('9');
        form.backspace();
        assert_eq!(form.price, "1.324");
        assert_eq!(parse_price(&form.price), Some(1.324));
        assert_eq!(parse_price("0"), None);
        assert_eq!(parse_price("1.2.3"), None);
    }

    #[test]
    fn the_preview_puts_asks_above_the_center_and_bars_track_size() {
        let levels = PassiveLadder {
            levels: 2,
            ..PassiveLadder::default()
        }
        .levels();
        let lines = ladder_lines(&levels, 1.0);
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("ask L2"));
        assert!(lines[1].starts_with("ask L1"));
        assert!(lines[2].contains("center"));
        assert!(lines[3].starts_with("bid L1"));
        // The innermost rung is the widest bar.
        let bar = |l: &str| l.matches('\u{2588}').count();
        assert_eq!(bar(&lines[3]), BAR_WIDTH);
        assert!(bar(&lines[4]) < BAR_WIDTH);
    }

    fn seat(idx: u32, leader: Pubkey) -> SectorSeat {
        SectorSeat {
            idx,
            seq: u64::from(idx) + 100,
            quote_authority: leader,
            frozen: false,
            tombstoned: false,
            quoting: true,
            stake: Default::default(),
        }
    }

    #[test]
    fn the_seat_pick_takes_the_one_live_vault_and_refuses_the_rest() {
        let leader = Pubkey::new_unique();
        // A tombstoned vault beside the live one does not count.
        let closed = SectorSeat {
            tombstoned: true,
            ..seat(0, leader)
        };
        let picked = pick_seat(vec![closed, seat(3, leader)], &leader).unwrap();
        assert_eq!(picked.idx, 3);
        let err = |seats| pick_seat(seats, &leader).unwrap_err().to_string();
        assert!(err(vec![closed]).contains("no live vault"));
        assert!(err(vec![seat(1, leader), seat(2, leader)]).contains("refusing to guess"));
        let frozen = SectorSeat {
            frozen: true,
            ..seat(1, leader)
        };
        assert!(err(vec![frozen]).contains("frozen"));
        let other = seat(1, Pubkey::new_unique());
        assert!(err(vec![other]).contains("quote authority"));
    }

    #[test]
    fn the_ticket_is_signed_by_the_quote_authority_alone() {
        // The hot path's two writes name the quote key as their signer, never
        // the leader — so a re-centre session needs no custody key.
        let quote = Keypair::new();
        let ticket = Ticket {
            op: PassiveOp::Deploy,
            market: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            base_symbol: "TGBP",
            quote_symbol: "USDC",
            seat: seat(2, quote.pubkey()),
            quote_authority: quote.pubkey(),
            levels: Vec::new(),
            center: 1.324,
            center_source: "typed",
            price: Price::from_value(1.324).unwrap(),
            warnings: Vec::new(),
        };
        assert!(ticket
            .summary()
            .iter()
            .any(|l| l.contains(&quote.pubkey().to_string()) && l.contains("quote authority")));
        // A wrong signing key is refused before any read or send — even with
        // the confirmed key as payer, so the check is on the signer argument.
        let stranger = Keypair::new();
        let client = RpcClient::new("http://127.0.0.1:1".to_string());
        let log = Logger::new(std::sync::mpsc::channel().0);
        let err = execute(&client, &quote, &stranger, Cluster::Localnet, &ticket, &log)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("quote authority is not the one confirmed"),
            "{err}"
        );
    }

    #[test]
    fn a_center_far_from_the_live_reference_is_warned() {
        assert_eq!(center_warning(1.33, 1.324, "live reference"), None);
        let warning = center_warning(13.24, 1.324, "live reference").unwrap();
        assert!(warning.contains("+900.0%"), "{warning}");
        assert!(warning.contains("live reference"), "{warning}");
        assert!(center_warning(1.20, 1.324, "estimator's fair").is_some());
    }

    /// A vault stamped at 1.0 an hour before `NOW`, posting a two-rung ladder
    /// a side — the price bits are whatever 1.0 encodes, since base and quote
    /// share decimals here.
    fn ladder_vault(flush: bool) -> Vault {
        let mut v = Vault::zeroed();
        let one = Price::from_value(1.0).unwrap().as_u32();
        v.reference_price.price = one.into();
        v.reference_price.quote_unix = ((NOW - 3_600) as u32).into();
        v.reference_price.stamp = (if flush {
            dropset_sdk::layout::FLUSH_BIT
        } else {
            0
        } | 7)
            .into();
        for i in 0..2 {
            for l in [&mut v.profile.asks[i], &mut v.profile.bids[i]] {
                l.size_bps = 2_500.into();
                l.expiry_offset_secs = WallSpan::UNBOUNDED.get().into();
                l.expiry_offset_slots = u32::MAX.into();
            }
            let p = Position {
                price: one.into(),
                size: 1_000_000.into(),
                expires_at_unix: u32::MAX.into(),
                expires_at_slot: u32::MAX.into(),
                ..Position::zeroed()
            };
            v.remaining.asks[i] = p;
            v.remaining.bids[i] = p;
        }
        v
    }

    const NOW: i64 = 1_800_000_000;
    const AT: (WallTime, SlotTime) = (WallTime::new(NOW as u32), SlotTime::new(1_000));
    const LEGS: Legs = Legs {
        base: Leg {
            symbol: "TGBP",
            decimals: 6,
        },
        quote: Leg {
            symbol: "USDC",
            decimals: 6,
        },
    };

    #[test]
    fn the_vault_view_names_each_flipped_rung_since_the_stamp() {
        let mut v = ladder_vault(false);
        // A taker lifted half of the inner ask: its proceeds rest as a bid.
        v.remaining.asks[0].size = 500_000.into();
        v.remaining.asks[0].flip_size = 662_000.into();
        v.remaining.asks[0].flip_price = Price::from_value(0.9998).unwrap().as_u32().into();
        let lines = ladder_state(&v, 4, &LEGS, AT);
        assert!(
            lines[0].starts_with("vault #4  stamped 1.0 h ago"),
            "{lines:?}"
        );
        assert_eq!(lines[1], "1 level flipped since the stamp:");
        // Outermost ask first, down to the outermost bid.
        assert_eq!(lines.len(), 2 + 4);
        assert!(lines[2].starts_with("ask L2"), "{lines:?}");
        assert!(lines[5].starts_with("bid L2"), "{lines:?}");
        let inner = &lines[3];
        assert!(
            inner.starts_with("ask L1") && inner.contains("rests 0.50 TGBP"),
            "{inner}"
        );
        assert!(inner.contains("flipped 0.66 USDC @ 0.9998"), "{inner}");
        assert!(!lines[2].contains("flipped"));
    }

    #[test]
    fn the_vault_view_reads_a_quiet_stamp_and_an_empty_profile() {
        let lines = ladder_state(&ladder_vault(true), 4, &LEGS, AT);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("rests as posted"), "{lines:?}");
        let mut empty = ladder_vault(false);
        empty.profile = Zeroable::zeroed();
        assert_eq!(
            ladder_state(&empty, 4, &LEGS, AT)[1],
            "no ladder rests on this vault"
        );
        // An expired rung is named as such, in either clock.
        let mut lapsed = ladder_vault(false);
        lapsed.remaining.bids[1].expires_at_unix = ((NOW - 1) as u32).into();
        lapsed.remaining.asks[1].expires_at_slot = 999.into();
        let lines = ladder_state(&lapsed, 4, &LEGS, AT);
        assert!(lines[2].ends_with("expired"), "{lines:?}");
        assert!(lines.last().unwrap().ends_with("expired"), "{lines:?}");
        assert!(!lines[3].ends_with("expired"), "{lines:?}");
        assert!(lines[1].contains("no level has flipped"), "{lines:?}");
    }

    #[test]
    fn a_one_sided_quiet_ladder_reads_its_life_from_the_side_it_posts() {
        // Bids alone: the unused ask rungs carry a zero (dead) life, which
        // must not read as the ladder having expired.
        let mut v = ladder_vault(true);
        v.profile.asks = Zeroable::zeroed();
        let lines = ladder_state(&v, 4, &LEGS, AT);
        assert!(lines[1].contains("rests as posted"), "{lines:?}");
        v.profile.bids[0].expiry_offset_secs = 60.into();
        v.profile.bids[1].expiry_offset_secs = 60.into();
        let lines = ladder_state(&v, 4, &LEGS, AT);
        assert!(lines[1].contains("has expired"), "{lines:?}");
    }

    /// [`ladder_vault`] with its two rungs a side at 5 and 10 bps.
    fn spaced_vault(flush: bool) -> Vault {
        let mut v = ladder_vault(flush);
        for (i, offset) in [500u32, 1_000].into_iter().enumerate() {
            v.profile.asks[i].price_offset_ppm = offset.into();
            v.profile.bids[i].price_offset_ppm = offset.into();
        }
        v
    }

    #[test]
    fn the_drift_names_the_fair_the_center_and_the_rungs_walked_past() {
        // The fair 7 bps above the 1.0 center: past the 5 bps ask, not the 10.
        let lines = drift_lines(&spaced_vault(true), &LEGS, &live_fair(1.0007), None);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].ends_with("gap +7.0 bps, past 1 of 2 rungs"),
            "{lines:?}"
        );
        assert!(
            lines[0].starts_with("fair 1.0007 · center 1.0000"),
            "{lines:?}"
        );
        // Below it the bids are the side walked through.
        let lines = drift_lines(&spaced_vault(true), &LEGS, &live_fair(0.998), None);
        assert!(
            lines[0].ends_with("gap -20.0 bps, past 2 of 2 rungs"),
            "{lines:?}"
        );
        // No fair: the center and why.
        let lines = drift_lines(&spaced_vault(true), &LEGS, &Fair::Off, None);
        assert_eq!(lines[0], "center 1.0000 · no fair-price database");
        // A dark vault has no center to measure from.
        let mut dark = spaced_vault(true);
        dark.reference_price.price = 0.into();
        let lines = drift_lines(&dark, &LEGS, &live_fair(1.0), None);
        assert_eq!(lines[0], "center  none stamped — the book is dark");
        // An empty profile is ladder_state's to report.
        let mut empty = spaced_vault(true);
        empty.profile = Zeroable::zeroed();
        assert!(drift_lines(&empty, &LEGS, &live_fair(1.0), None).is_empty());
    }

    #[test]
    fn the_drift_shows_the_spot_beside_the_fair_and_an_emptied_side() {
        let spot = Spot {
            price: 1.0002,
            age_secs: 40,
        };
        let lines = drift_lines(&spaced_vault(true), &LEGS, &live_fair(1.0), Some(spot));
        assert_eq!(lines[1], "orca spot 1.0002 (+2.0 bps vs fair, 40 s old)");
        let lines = drift_lines(&spaced_vault(true), &LEGS, &Fair::Off, Some(spot));
        assert_eq!(lines[1], "orca spot 1.0002 (40 s old)");
        // Every ask lifted since the stamp: the book is one-sided.
        let mut lifted = spaced_vault(false);
        for p in &mut lifted.remaining.asks {
            p.size = 0.into();
        }
        let lines = drift_lines(&lifted, &LEGS, &Fair::Off, None);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].starts_with("no ask rests"), "{lines:?}");
        let mut hit = spaced_vault(false);
        for p in &mut hit.remaining.bids {
            p.size = 0.into();
        }
        let lines = drift_lines(&hit, &LEGS, &Fair::Off, None);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].starts_with("no bid rests"), "{lines:?}");
        // Before the stamp's flush everything rests as posted.
        let mut armed = spaced_vault(true);
        armed.remaining.asks = Zeroable::zeroed();
        assert_eq!(drift_lines(&armed, &LEGS, &Fair::Off, None).len(), 1);
    }

    fn live_fair(price: f64) -> Fair {
        Fair::Live {
            product: "TGBP-USDC",
            price,
            age_secs: 2,
            regime: "normal".into(),
        }
    }

    #[test]
    fn a_governing_fair_is_the_blank_center_and_a_paused_one_refuses() {
        let (live, mid) = (Some(1.30), Some(1.31));
        // A typed price beats every source.
        assert_eq!(
            resolve_center("1.5", live, mid, &live_fair(1.32)).unwrap(),
            (1.5, "typed")
        );
        assert!(resolve_center("1.x", live, mid, &live_fair(1.32)).is_err());
        // A live fair is the blank center, over the reference and the mid.
        assert_eq!(
            resolve_center("", live, mid, &live_fair(1.32)).unwrap(),
            (1.32, "the estimator's fair")
        );
        // A paused or unusable fair refuses — no fallback to the reference.
        let paused = Fair::Paused {
            product: "TGBP-USDC",
            regime: "paused".into(),
        };
        let err = resolve_center("", live, mid, &paused)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("paused") && err.contains("type a price"),
            "{err}"
        );
        assert!(resolve_center("", live, mid, &Fair::Unusable("stale".into())).is_err());
        // No estimator to ask: the reference, then the mid, then nothing.
        for none in [Fair::Off, Fair::Unpublished] {
            assert_eq!(resolve_center("", live, mid, &none).unwrap().0, 1.30);
            assert_eq!(resolve_center("", None, mid, &none).unwrap().0, 1.31);
            assert!(resolve_center("", None, None, &none).is_err());
        }
        // The form says the same before the ticket is built.
        let mut form = Form::new(PassiveOp::Recenter);
        form.fair = paused;
        assert!(form.lines()[0].contains("type one — the estimator is paused"));
        form.fair = live_fair(1.32);
        let line = &form.lines()[0];
        assert!(line.ends_with("fair 1.3200 (TGBP-USDC, 2 s old)"), "{line}");
        form.fair = Fair::Unpublished;
        assert!(form.lines()[0].contains("live reference"));
    }

    #[test]
    fn the_confirmation_names_the_expiry_and_every_rung() {
        let leader = Pubkey::new_unique();
        let levels = PassiveLadder {
            levels: 3,
            expiry_secs: EXPIRY_PRESETS[2],
            ..PassiveLadder::default()
        }
        .levels();
        let ticket = Ticket {
            op: PassiveOp::Deploy,
            market: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            base_symbol: "TGBP",
            quote_symbol: "USDC",
            seat: seat(2, leader),
            quote_authority: leader,
            levels,
            center: 1.324,
            center_source: "typed",
            price: Price::from_value(1.324).unwrap(),
            warnings: vec!["check me".to_string()],
        };
        let lines = ticket.summary();
        assert!(lines[0].contains("TGBP/USDC  vault #102"));
        assert!(lines.iter().any(|l| l == "expiry  24 h"));
        assert_eq!(lines.iter().filter(|l| l.contains(" L")).count(), 6);
        assert!(lines.iter().any(|l| l == "warning: check me"));
        // A re-center previews no rungs.
        let recenter = Ticket {
            op: PassiveOp::Recenter,
            levels: Vec::new(),
            ..ticket
        };
        assert!(!recenter.summary().iter().any(|l| l.starts_with("expiry")));
    }
}
