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
//! ladder comes from [`dropset_sdk::ladder::PassiveLadder`], the builder the
//! maker bot's passive mode uses, so the two cannot drift on the shape rules.
//!
//! Both writes are the vault's quote authority's to sign, so the vault is
//! found by that key and the leader — the custody key — is never needed: a
//! daily visit loads only the quote key.
//!
//! The stop is not here: the emergency kill (`K` on mainnet) zeroes every
//! level on the quote key's vaults, passive ones included.

use crate::accounts::{self, MarketView};
use crate::book;
use crate::chain;
use crate::cluster::Cluster;
use crate::emergency::{self, SectorSeat};
use crate::job::Logger;
use crate::market;
use anyhow::{bail, Context, Result};
use dropset_sdk::clock::WallSpan;
use dropset_sdk::ladder::{
    build_profile, passive_shape_warnings, LadderLevel, PassiveLadder, PassiveShape,
};
use dropset_sdk::layout::N_LEVELS;
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
    Spacing,
    Levels,
    Shape,
    FlipSpread,
    Expiry,
}

const DEPLOY_FIELDS: [Field; 6] = [
    Field::Price,
    Field::Spacing,
    Field::Levels,
    Field::Shape,
    Field::FlipSpread,
    Field::Expiry,
];
const RECENTER_FIELDS: [Field; 1] = [Field::Price];

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub op: PassiveOp,
    pub ladder: PassiveLadder,
    /// Typed human quote-per-base; blank re-stamps the vault's live
    /// reference, or centers on the book mid when it has none.
    pub price: String,
    /// Index into [`Form::fields`] of the line being edited.
    pub cursor: usize,
}

impl Form {
    pub fn new(op: PassiveOp) -> Self {
        Self {
            op,
            ladder: PassiveLadder::default(),
            price: String::new(),
            cursor: 0,
        }
    }

    pub fn fields(&self) -> &'static [Field] {
        match self.op {
            PassiveOp::Deploy => &DEPLOY_FIELDS,
            PassiveOp::Recenter => &RECENTER_FIELDS,
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
        let l = &mut self.ladder;
        let up = delta > 0;
        match field {
            Field::Price => {}
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
            Field::Shape => {
                l.shape = match l.shape {
                    PassiveShape::Bell => PassiveShape::Uniform,
                    PassiveShape::Uniform => PassiveShape::Bell,
                };
            }
            Field::FlipSpread => {
                l.flip_spread_ppm =
                    stepped(l.flip_spread_ppm, FLIP_STEP_PPM, up).min(MAX_FLIP_SPREAD_PPM);
            }
            Field::Expiry => {
                let at = EXPIRY_PRESETS
                    .iter()
                    .position(|e| *e == l.expiry_secs)
                    .unwrap_or(0);
                let next = if up {
                    (at + 1).min(EXPIRY_PRESETS.len() - 1)
                } else {
                    at.saturating_sub(1)
                };
                l.expiry_secs = EXPIRY_PRESETS[next];
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
        self.fields()
            .iter()
            .enumerate()
            .map(|(i, field)| {
                let value = match field {
                    Field::Price if self.price.is_empty() => {
                        "live reference (or book mid)".to_string()
                    }
                    Field::Price => self.price.clone(),
                    Field::Spacing => bps(l.spacing_ppm),
                    Field::Levels => format!("{} a side", l.levels),
                    Field::Shape => match l.shape {
                        PassiveShape::Bell => "bell".to_string(),
                        PassiveShape::Uniform => "uniform".to_string(),
                    },
                    Field::FlipSpread => bps(l.flip_spread_ppm),
                    Field::Expiry => expiry_label(l.expiry_secs),
                };
                let name = match field {
                    Field::Price => "center price",
                    Field::Spacing => "spacing",
                    Field::Levels => "levels",
                    Field::Shape => "shape",
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
/// ticket `form` describes. Sends nothing.
pub fn prepare(
    client: &RpcClient,
    repo_root: &Path,
    cluster: Cluster,
    market: &MarketView,
    quote_authority: &Pubkey,
    form: &Form,
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
    let (center, center_source) = match (parse_price(&form.price), live) {
        (Some(typed), _) => (typed, "typed"),
        _ if !form.price.is_empty() => bail!("enter a positive number, e.g. 1.324"),
        // A blank re-stamp is the daily keep-alive: same center, fresh stamp.
        (None, Some(live)) => (live, "the vault's live reference, re-stamped"),
        // Only a two-sided book has a mid; one side alone is not a center.
        (None, None) if !market.asks.is_empty() && !market.bids.is_empty() => {
            let mid = book::mid_price(market).context("the book has no mid")?;
            (mid, "the book mid")
        }
        (None, None) => {
            bail!("the vault has no reference and the book is not two-sided — type a price")
        }
    };
    let ratio = human_to_atoms_ratio(center, market.base_decimals, market.quote_decimals);
    let price = Price::from_value(ratio).with_context(|| format!("encode the center {center}"))?;
    let (levels, mut warnings) = match form.op {
        PassiveOp::Deploy => {
            let levels = form.ladder.levels();
            let warnings = passive_shape_warnings(&levels);
            (levels, warnings)
        }
        PassiveOp::Recenter => (Vec::new(), Vec::new()),
    };
    warnings.extend(live.and_then(|live| center_warning(center, live)));
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

/// A warning when `center` sits more than [`CENTER_DEVIATION_WARN`] from the
/// vault's `live` reference — the one check between a mistyped digit and a
/// book that is swept on arrival.
fn center_warning(center: f64, live: f64) -> Option<String> {
    let off = center / live - 1.0;
    (off.abs() > CENTER_DEVIATION_WARN).then(|| {
        format!(
            "the center is {:+.1}% from the live reference {} — check the digits",
            off * 100.0,
            book::fmt_price(live)
        )
    })
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
            ticket.quote_authority,
            ticket.market,
            ticket.seat.idx,
            profile_bytes(&profile),
        ));
    }
    let slot = client.get_slot().context("current slot")?;
    ixs.push(set_reference_price_ix(
        ticket.quote_authority,
        ticket.market,
        ticket.seat.idx,
        ticket.price,
        slot,
        dropset_sdk::time::now_unix(),
    ));
    let ixs: Vec<_> = ixs
        .into_iter()
        .map(|ix| chain::retarget(cluster, ix))
        .collect();
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

    #[test]
    fn the_deploy_form_starts_on_the_shared_default_ladder() {
        let form = Form::new(PassiveOp::Deploy);
        assert_eq!(form.ladder, PassiveLadder::default());
        assert_eq!(form.field(), Field::Price);
        assert_eq!(form.fields().len(), DEPLOY_FIELDS.len());
        // A re-center edits the price alone.
        assert_eq!(Form::new(PassiveOp::Recenter).fields(), &[Field::Price]);
    }

    #[test]
    fn stepping_clamps_every_field_to_its_range() {
        let mut form = Form::new(PassiveOp::Deploy);
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
        form.step(1);
        assert_eq!(form.ladder.shape, PassiveShape::Uniform);
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
        assert_eq!(center_warning(1.33, 1.324), None);
        let warning = center_warning(13.24, 1.324).unwrap();
        assert!(warning.contains("+900.0%"), "{warning}");
        assert!(center_warning(1.20, 1.324).is_some());
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
