//! Order-book pane rendering.
//!
//! Reconstructs a price ladder from the resting book the shared SDK matcher
//! returns ([`crate::accounts::MarketView::asks`] / `bids`) and renders it in
//! the style of the dropset-alpha terminal book: asks above a mid/spread
//! divider (red), bids below (green), each row a right-aligned price and a
//! horizontal depth bar proportional to its size, with the size in human
//! units trailing in a faded column. Pure formatting — the data is decoded
//! at poll time, so drawing never touches the chain.

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

    let mut out = Vec::with_capacity(asks.len() + bids.len() + 1);
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

/// One book row: right-aligned price · depth bar · faded human size, all on
/// the side's color.
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
        Span::styled(format!("{:>10.4}", r.price), Style::new().fg(color)),
        Span::raw("  "),
        Span::styled(format!("{bar:<BAR_WIDTH$}"), Style::new().fg(color)),
        Span::styled(
            format!("  {:>12.2}", r.size),
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
            format!("\u{2500}\u{2500} mid {mid:.4}  \u{b7}  spread {spread_bps:.1} bps \u{2500}\u{2500}")
        }
        (Some(a), None) => format!("\u{2500}\u{2500} best ask {:.4} \u{2500}\u{2500}", a.price),
        (None, Some(b)) => format!("\u{2500}\u{2500} best bid {:.4} \u{2500}\u{2500}", b.price),
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
        // two asks + divider + one bid
        assert_eq!(out.len(), 4);
        // Highest ask at the top; best (lowest) ask just above the divider.
        assert!(text(&out[0]).contains("0.7500"));
        assert!(text(&out[1]).contains("0.7400"));
        assert!(text(&out[2]).contains("mid"));
        assert!(text(&out[3]).contains("0.7200"));
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
        // one ask row + a one-sided divider
        assert_eq!(out.len(), 2);
        assert!(text(&out[0]).contains("1.50"));
        assert!(text(&out[1]).contains("best ask"));
    }
}
