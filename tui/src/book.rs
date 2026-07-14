//! Order-book pane rendering.
//!
//! Reconstructs a price ladder from the resting book the shared SDK matcher
//! returns ([`crate::accounts::MarketView::asks`] / `bids`) and renders it in
//! the style of the dropset-alpha terminal book: a column header, then asks
//! above a mid/spread divider (red), bids below (green), each row a horizontal
//! depth bar that grows right-to-left toward a right-aligned price, then the
//! size in human units and a quote-denominated volume (price × size) trailing
//! in two faded columns. Pure formatting — the data is decoded at poll time,
//! so drawing never touches the chain.

use crate::accounts::MarketView;
use dropset_sdk::matching::BookLevel;
use dropset_sdk::price::Price;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Bar width in cells at full (max) depth.
const BAR_WIDTH: usize = 16;
/// Maximum price levels rendered per side.
const MAX_LEVELS: usize = 8;
/// dropset-alpha's ask (sell) red and bid (buy) green.
const ASK_COLOR: Color = Color::Rgb(240, 75, 90);
const BID_COLOR: Color = Color::Rgb(30, 135, 80);
/// Widest the adaptive price render (`fmt_price`) may grow: its decimal count
/// is clamped to this ceiling, and [`PRICE_WIDTH`] is derived from it, so the
/// rendered precision and the column that holds it can't drift apart. FX prices
/// carry a single integer digit, so the worst-case render is `0.` plus this
/// many decimals.
const MAX_PRICE_DECIMALS: usize = 10;
/// Price-column width: `0.` (two chars) plus the [`MAX_PRICE_DECIMALS`] ceiling.
const PRICE_WIDTH: usize = 2 + MAX_PRICE_DECIMALS;

/// A display row: human price (quote per base) and human size (base units).
struct Row {
    price: f64,
    size: f64,
}

/// Render `market`'s order book into styled lines: asks (red, highest at the
/// top, best just above the divider), a mid/spread divider, then bids (green,
/// best just below it). Returns a single placeholder line when the book holds
/// no resting liquidity.
pub fn lines(market: &MarketView) -> Vec<Line<'static>> {
    let asks = ladder(&market.asks, market.base_decimals, market.quote_decimals);
    let bids = ladder(&market.bids, market.base_decimals, market.quote_decimals);

    if asks.is_empty() && bids.is_empty() {
        return vec![Line::from(Span::styled(
            "  no resting liquidity",
            Style::new().fg(Color::DarkGray),
        ))];
    }

    // Scale every bar against the deepest level on either side, so bid and
    // ask depth are directly comparable across the divider.
    let max_size = asks
        .iter()
        .chain(bids.iter())
        .map(|r| r.size)
        .fold(0.0_f64, f64::max);

    let mut out = Vec::with_capacity(asks.len() + bids.len() + 2);
    // A column header over the ladder, like a common exchange book.
    out.push(header_line());
    // Asks ascend best-first, so render highest first to seat the best ask
    // just above the divider.
    for r in asks.iter().rev() {
        out.push(row_line(r, max_size, ASK_COLOR));
    }
    out.push(divider(asks.first(), bids.first()));
    // Bids descend best-first — render as-is, best just below the divider.
    for r in &bids {
        out.push(row_line(r, max_size, BID_COLOR));
    }
    out
}

/// Format a human price with adaptive precision — roughly four significant
/// figures whatever the magnitude — so a small-value token (IDRX ≈ 0.000056)
/// keeps its figures instead of collapsing to a single `0.0001`, while a
/// ~1-valued token (EURC 1.14) still reads cleanly. The decimal count grows as
/// the price shrinks, clamped to a sane range.
pub fn fmt_price(p: f64) -> String {
    if !p.is_finite() || p <= 0.0 {
        return format!("{p:.4}");
    }
    // 10^exp ≤ p < 10^(exp+1). Four significant figures ⇒ (3 − exp) decimals,
    // with a four-decimal floor so ~1-valued tokens still read as e.g. 1.1400
    // while a sub-cent token grows its decimals to keep its figures.
    let exp = p.log10().floor() as i32;
    let decimals = (3 - exp).clamp(4, MAX_PRICE_DECIMALS as i32) as usize;
    format!("{p:.decimals$}")
}

/// The mid price (quote per base, human units) from the best bid and ask —
/// their average, or whichever side alone has liquidity, and `None` for an
/// empty book. Drives the markets list's per-market price / idle indicator.
pub fn mid_price(market: &MarketView) -> Option<f64> {
    let best = |levels: &[BookLevel]| {
        levels
            .first()
            .map(|l| human_price(l.price, market.base_decimals, market.quote_decimals))
    };
    match (best(&market.asks), best(&market.bids)) {
        (Some(a), Some(b)) => Some((a + b) / 2.0),
        (Some(p), None) | (None, Some(p)) => Some(p),
        (None, None) => None,
    }
}

/// A resting level's price in human quote-per-base units: quote atoms for one
/// whole base unit, de-scaled by the quote mint's decimals.
fn human_price(price: Price, base_dec: u8, quote_dec: u8) -> f64 {
    price.quote_for_base(10u64.pow(base_dec as u32)) as f64 / 10f64.powi(quote_dec as i32)
}

/// Aggregate the raw best-first `levels` into at most [`MAX_LEVELS`] display
/// rows — summing sizes that share a price (adjacent after the price-time
/// sort) — and scale atoms to human units.
fn ladder(levels: &[BookLevel], base_dec: u8, quote_dec: u8) -> Vec<Row> {
    let base_scale = 10f64.powi(base_dec as i32);
    let mut rows: Vec<Row> = Vec::new();
    // Track the previous level's on-chain price so equal-priced levels merge
    // on exact `Price` equality, not a float tolerance — levels are
    // price-sorted, so equal prices are adjacent.
    let mut prev: Option<Price> = None;
    for lvl in levels {
        let price = human_price(lvl.price, base_dec, quote_dec);
        let size = lvl.size as f64 / base_scale;
        if prev == Some(lvl.price) {
            // Same price as the level above — fold its depth into that row.
            if let Some(last) = rows.last_mut() {
                last.size += size;
            }
        } else {
            if rows.len() == MAX_LEVELS {
                break;
            }
            rows.push(Row { price, size });
            prev = Some(lvl.price);
        }
    }
    rows
}

/// The column header over the ladder — a `depth · price · size · volume` banner
/// in the style of a common exchange book, its widths matching [`row_line`] so
/// the labels sit over their columns.
fn header_line() -> Line<'static> {
    let style = Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(format!("{:>BAR_WIDTH$}", "depth"), style),
        Span::styled(format!("  {:>PRICE_WIDTH$}", "price"), style),
        Span::styled(format!("  {:>12}", "size"), style),
        Span::styled(format!("  {:>12}", "volume"), style),
    ])
}

/// One book row: a depth bar that grows right-to-left toward the price column ·
/// right-aligned price · faded human size · the quote-denominated volume
/// (price × size). The bar is right-aligned so deeper levels reach further
/// left, seating the freshest depth against the price.
fn row_line(r: &Row, max_size: f64, color: Color) -> Line<'static> {
    let scaled = if max_size > 0.0 {
        ((r.size / max_size) * BAR_WIDTH as f64).round() as usize
    } else {
        0
    };
    // At least one cell for any non-zero level, so thin depth still shows.
    let filled = scaled.clamp(usize::from(r.size > 0.0), BAR_WIDTH);
    let bar = "\u{2588}".repeat(filled);
    Line::from(vec![
        Span::styled(format!("{bar:>BAR_WIDTH$}"), Style::new().fg(color)),
        Span::styled(
            format!("  {:>PRICE_WIDTH$}", fmt_price(r.price)),
            Style::new().fg(color),
        ),
        Span::styled(
            format!("  {:>12.2}", r.size),
            Style::new().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("  {:>12.2}", r.price * r.size),
            Style::new().fg(Color::DarkGray),
        ),
    ])
}

/// The mid/spread divider between asks and bids, from the best level on each
/// side (or a one-sided label when only one side has liquidity).
fn divider(best_ask: Option<&Row>, best_bid: Option<&Row>) -> Line<'static> {
    let label = match (best_ask, best_bid) {
        (Some(a), Some(b)) => {
            let mid = (a.price + b.price) / 2.0;
            let spread_bps = if mid > 0.0 {
                (a.price - b.price) / mid * 10_000.0
            } else {
                0.0
            };
            format!(
                "\u{2500}\u{2500} mid {}  \u{b7}  spread {spread_bps:.1} bps \u{2500}\u{2500}",
                fmt_price(mid)
            )
        }
        (Some(a), None) => {
            format!(
                "\u{2500}\u{2500} best ask {} \u{2500}\u{2500}",
                fmt_price(a.price)
            )
        }
        (None, Some(b)) => {
            format!(
                "\u{2500}\u{2500} best bid {} \u{2500}\u{2500}",
                fmt_price(b.price)
            )
        }
        (None, None) => "\u{2500}\u{2500}".to_string(),
    };
    Line::from(Span::styled(
        label,
        Style::new().fg(Color::Gray).add_modifier(Modifier::DIM),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::MarketView;
    use dropset_sdk::price::Price;
    use solana_pubkey::Pubkey;

    /// A `BookLevel` at a human price (quote per base) and atom size.
    fn lvl(price: f64, size: u64) -> BookLevel {
        BookLevel {
            price: Price::from_value(price).unwrap(),
            size,
        }
    }

    /// A minimal 6/6-decimal market carrying just the book the pane reads.
    fn market(asks: Vec<BookLevel>, bids: Vec<BookLevel>) -> MarketView {
        MarketView {
            address: Pubkey::default(),
            lamports: 0,
            base_mint: Pubkey::default(),
            quote_mint: Pubkey::default(),
            base_treasury: Pubkey::default(),
            quote_treasury: Pubkey::default(),
            base_treasury_lamports: 0,
            quote_treasury_lamports: 0,
            active_count: 1,
            live_vaults: Vec::new(),
            leader_quote_slot: None,
            reference_price: None,
            depositors: Vec::new(),
            base_decimals: 6,
            quote_decimals: 6,
            asks,
            bids,
        }
    }

    /// Flatten a line's spans back to plain text for assertions.
    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn fmt_price_keeps_significant_figures_across_magnitudes() {
        // ~1-valued tokens read cleanly at four decimals.
        assert_eq!(fmt_price(1.14), "1.1400");
        assert_eq!(fmt_price(0.7705), "0.7705");
        // A small-value token keeps its figures instead of collapsing to a
        // single 0.0001 (the IDRX case).
        assert_eq!(fmt_price(0.000056), "0.00005600");
        assert_ne!(fmt_price(0.000056), "0.0001");
        // Sentinels / non-finite fall back to a plain four-decimal render.
        assert_eq!(fmt_price(0.0), "0.0000");
    }

    #[test]
    fn fmt_price_never_overflows_the_price_column() {
        // The decimal count is clamped to MAX_PRICE_DECIMALS and PRICE_WIDTH is
        // derived from it, so a sub-cent FX price (single integer digit) must
        // always fit the column that holds it — no misaligned size/volume.
        let smallest = 1e-9; // well below any real token, exercises the ceiling.
        assert!(fmt_price(smallest).len() <= PRICE_WIDTH);
        // The clamp caps decimals: even 1e-9 renders at MAX_PRICE_DECIMALS, not
        // the 12 decimals its magnitude would otherwise ask for.
        assert_eq!(
            fmt_price(smallest),
            format!("{smallest:.MAX_PRICE_DECIMALS$}")
        );
    }

    #[test]
    fn empty_book_shows_a_placeholder() {
        let out = lines(&market(Vec::new(), Vec::new()));
        assert_eq!(out.len(), 1);
        assert!(text(&out[0]).contains("no resting liquidity"));
    }

    #[test]
    fn asks_sit_above_a_mid_divider_above_bids() {
        // Asks arrive best-first (ascending); one bid below.
        let out = lines(&market(
            vec![lvl(0.74, 1_000_000), lvl(0.75, 500_000)],
            vec![lvl(0.72, 2_000_000)],
        ));
        // header + two asks + divider + one bid
        assert_eq!(out.len(), 5);
        assert!(text(&out[0]).contains("price"));
        // Highest ask at the top; best (lowest) ask just above the divider.
        assert!(text(&out[1]).contains("0.7500"));
        assert!(text(&out[2]).contains("0.7400"));
        assert!(text(&out[3]).contains("mid"));
        assert!(text(&out[4]).contains("0.7200"));
    }

    #[test]
    fn rows_lead_with_the_right_to_left_depth_bar() {
        let out = lines(&market(vec![lvl(0.75, 1_000_000)], Vec::new()));
        // header + one ask + one-sided divider.
        assert_eq!(out.len(), 3);
        // The header names its columns depth · price · size · volume, in order.
        let header = text(&out[0]);
        let depth_at = header.find("depth").unwrap();
        let price_at = header.find("price").unwrap();
        let size_at = header.find("size").unwrap();
        let volume_at = header.find("volume").unwrap();
        assert!(depth_at < price_at && price_at < size_at && size_at < volume_at);
        // The row leads with the full-block depth bar (right-aligned, so it
        // grows right-to-left), then price, size, and the price×size volume.
        let row = &out[1];
        assert_eq!(row.spans.len(), 4);
        assert!(row.spans[0].content.contains('\u{2588}'));
        assert!(row.spans[1].content.contains("0.7500"));
        // volume = price × size = 0.75 × 1.0 = 0.75.
        assert!(row.spans[3].content.contains("0.75"));
    }

    #[test]
    fn mid_price_averages_best_bid_and_ask_and_handles_one_sided() {
        // Both sides → average of the best of each.
        let both = market(vec![lvl(0.75, 1), lvl(0.76, 1)], vec![lvl(0.73, 1)]);
        assert!((mid_price(&both).unwrap() - 0.74).abs() < 1e-9);
        // One side only → that side's best price.
        let asks_only = market(vec![lvl(0.75, 1)], Vec::new());
        assert!((mid_price(&asks_only).unwrap() - 0.75).abs() < 1e-9);
        // Empty book → no mid.
        assert!(mid_price(&market(Vec::new(), Vec::new())).is_none());
    }

    #[test]
    fn equal_priced_levels_aggregate() {
        // Two asks at one price collapse to a single row summing their size.
        let out = lines(&market(
            vec![lvl(0.74, 1_000_000), lvl(0.74, 500_000)],
            Vec::new(),
        ));
        // header + one ask row + a one-sided divider
        assert_eq!(out.len(), 3);
        assert!(text(&out[1]).contains("1.50"));
        assert!(text(&out[2]).contains("best ask"));
    }
}
