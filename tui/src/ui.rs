//! Pure rendering of the dashboard from [`App`] state.
//!
//! One screen: a status bar (validator + derived phase + wallet balance), a
//! left action menu (enabled / greyed with reasons, recommended next step
//! marked), a right account table (✓ / ✗ + lamports), and a scrolling log.

use crate::accounts::{ChainState, ParticipantView, Phase};
use crate::action::{self, Action};
use crate::app::{App, LogKind};
use crate::book;
use crate::explorer;
use dropset_sdk::DROPSET_ID;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use std::sync::atomic::Ordering;

/// Render the whole dashboard.
pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    // Rebuilt every frame from the current layout: stale rectangles from a
    // prior size/state must not catch clicks.
    app.click_targets.clear();
    let area = f.area();
    let [status, body, log, footer] = Layout::new(
        Direction::Vertical,
        [
            Constraint::Length(3),
            Constraint::Percentage(60),
            Constraint::Min(6),
            Constraint::Length(3),
        ],
    )
    .areas(area);

    draw_status(f, app, status);

    // Three columns: the action menu, a stacked accounts + CU column, and the
    // order book.
    let [menu_area, mid_area, book_area] = Layout::new(
        Direction::Horizontal,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(32),
            Constraint::Percentage(38),
        ],
    )
    .areas(body);
    draw_menu(f, app, menu_area);
    let [accounts_area, cu_area] = Layout::new(
        Direction::Vertical,
        [Constraint::Percentage(62), Constraint::Percentage(38)],
    )
    .areas(mid_area);
    draw_accounts(f, app, accounts_area);
    draw_cu(f, app, cu_area);
    draw_book(f, app, book_area);

    draw_log(f, app, log);

    let help = Paragraph::new(
        "j/k move  ·  enter / 1-9 run  ·  click account → explorer  ·  r refresh  ·  q quit",
    )
    .block(Block::default().borders(Borders::ALL))
    .alignment(Alignment::Center);
    f.render_widget(help, footer);
}

fn draw_status(f: &mut Frame<'_>, app: &App, area: Rect) {
    let phase = app.chain.phase();
    let (phase_color, _) = phase_style(phase);

    let validator = if app.chain.validator_up {
        Span::styled(
            format!("RUNNING (slot {})", app.chain.slot.unwrap_or(0)),
            Style::new().fg(Color::Green),
        )
    } else {
        Span::styled("starting…", Style::new().fg(Color::Yellow))
    };

    let line = Line::from(vec![
        Span::styled(
            "dropset localnet",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  ·  validator "),
        validator,
        Span::raw("  ·  phase "),
        Span::styled(
            phase.label(),
            Style::new().fg(phase_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  ·  wallet {:.3} SOL",
            app.chain.wallet_lamports as f64 / LAMPORTS_PER_SOL as f64
        )),
        Span::raw("  ·  explorer "),
        explorer_status(app),
    ]);
    let job = if app.job_running {
        " (job running) "
    } else {
        ""
    };
    f.render_widget(
        Paragraph::new(line).block(
            Block::default()
                .title(format!(" control plane{job}"))
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// Colored status-bar span for the managed explorer container.
fn explorer_status(app: &App) -> Span<'static> {
    let s = app.ctx.explorer_state.load(Ordering::SeqCst);
    let color = match s {
        explorer::state::READY => Color::Green,
        explorer::state::STARTING => Color::Yellow,
        explorer::state::FAILED => Color::Red,
        _ => Color::DarkGray,
    };
    Span::styled(explorer::state_label(s), Style::new().fg(color))
}

fn draw_menu(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let phase = app.chain.phase();
    let next = action::recommended_next(phase);
    let items: Vec<ListItem> = action::MENU
        .iter()
        .enumerate()
        .map(|(i, &a)| menu_item(i, a, phase, next))
        .collect();
    let list = List::new(items)
        .block(Block::default().title(" actions ").borders(Borders::ALL))
        .highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_stateful_widget(list, area, &mut app.menu);
}

fn menu_item(i: usize, action: Action, phase: Phase, next: Option<Action>) -> ListItem<'static> {
    let enabled = action.enabled(phase);
    let recommended = next == Some(action);
    let key = Span::styled(format!("{}. ", i + 1), Style::new().fg(Color::DarkGray));
    let label_style = if !enabled {
        Style::new().fg(Color::DarkGray)
    } else if recommended {
        Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let mut spans = vec![key, Span::styled(action.label().to_string(), label_style)];
    if recommended {
        spans.push(Span::styled("  ← next", Style::new().fg(Color::Green)));
    } else if !enabled {
        spans.push(Span::styled(
            format!("  ({})", action.disabled_reason(phase)),
            Style::new().fg(Color::DarkGray),
        ));
    }
    ListItem::new(Line::from(spans))
}

fn draw_accounts(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    let rows = build_account_rows(&app.chain, &app.mint_symbols);
    // Register each address-bearing row as a click target over its inner-row
    // rect, so a left-click anywhere on the row opens that account in the
    // explorer. The inner area sits one cell in from the border on every side.
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let inner_w = area.width.saturating_sub(2);
    let max_y = area.y + area.height.saturating_sub(1);
    for (i, (_, addr)) in rows.iter().enumerate() {
        let y = inner_y + i as u16;
        if y >= max_y {
            break; // past the bottom border — not drawn, so not clickable
        }
        if let Some(addr) = addr {
            app.click_targets.push((
                Rect {
                    x: inner_x,
                    y,
                    width: inner_w,
                    height: 1,
                },
                *addr,
            ));
        }
    }
    let lines: Vec<Line> = rows.into_iter().map(|(line, _)| line).collect();
    f.render_widget(
        Paragraph::new(lines).block(Block::default().title(" accounts ").borders(Borders::ALL)),
        area,
    );
}

/// Build the accounts-pane rows from chain state: each row is a rendered line
/// paired with the address it points at (if any). The pairing lets the caller
/// both draw the line and register it as a click target that opens that
/// address in the explorer. `symbols` maps a mint to its ticker, so the
/// participant rows name their actual coins rather than generic base/quote.
fn build_account_rows(
    chain: &ChainState,
    symbols: &[(Pubkey, &'static str)],
) -> Vec<(Line<'static>, Option<Pubkey>)> {
    // The program id is fixed and meaningful even before deploy, so its row is
    // always clickable; absent registry/market rows have no real address.
    let mut rows: Vec<(Line<'static>, Option<Pubkey>)> = vec![(
        account_line("program", chain.program_deployed, &DROPSET_ID, None),
        Some(DROPSET_ID),
    )];

    match &chain.registry {
        Some(reg) => {
            rows.push((
                account_line("registry", true, &reg.address, Some(reg.lamports)),
                Some(reg.address),
            ));
            rows.push((
                account_line(
                    "fee vault",
                    true,
                    &reg.fee_vault,
                    Some(reg.fee_vault_lamports),
                ),
                Some(reg.fee_vault),
            ));
            rows.push((count_line(format!("  markets: {}", reg.market_count)), None));
        }
        None => rows.push((
            account_line("registry", false, &Pubkey::default(), None),
            None,
        )),
    }

    match &chain.market {
        Some(mkt) => {
            rows.push((
                account_line("market", true, &mkt.address, Some(mkt.lamports)),
                Some(mkt.address),
            ));
            rows.push((
                account_line(
                    "base treasury",
                    true,
                    &mkt.base_treasury,
                    Some(mkt.base_treasury_lamports),
                ),
                Some(mkt.base_treasury),
            ));
            rows.push((
                account_line(
                    "quote treasury",
                    true,
                    &mkt.quote_treasury,
                    Some(mkt.quote_treasury_lamports),
                ),
                Some(mkt.quote_treasury),
            ));
            rows.push((
                count_line(format!("  active vaults: {}", mkt.active_count)),
                None,
            ));
            // The market's participants — the MM bot (vault leader) and the
            // swapper — with the token holdings in their own wallets, labelled
            // with the market's actual coin tickers.
            let base_symbol = symbol_for(symbols, &mkt.base_mint, "base");
            let quote_symbol = symbol_for(symbols, &mkt.quote_mint, "quote");
            if let Some(leader) = &chain.leader {
                rows.push((
                    participant_line(
                        "leader",
                        leader,
                        mkt.base_decimals,
                        mkt.quote_decimals,
                        base_symbol,
                        quote_symbol,
                    ),
                    Some(leader.address),
                ));
            }
            if let Some(swapper) = &chain.swapper {
                rows.push((
                    participant_line(
                        "swapper",
                        swapper,
                        mkt.base_decimals,
                        mkt.quote_decimals,
                        base_symbol,
                        quote_symbol,
                    ),
                    Some(swapper.address),
                ));
            }
        }
        None => rows.push((
            account_line("market", false, &Pubkey::default(), None),
            None,
        )),
    }

    rows
}

/// A dimmed, non-address summary row (e.g. `markets: 1`).
fn count_line(text: String) -> Line<'static> {
    Line::from(Span::styled(text, Style::new().fg(Color::DarkGray)))
}

/// The ticker for `mint` from the known-mint map, or `fallback` if the mint
/// isn't one the bootstrap knows about (e.g. a market minted outside it).
fn symbol_for<'a>(
    symbols: &'a [(Pubkey, &'static str)],
    mint: &Pubkey,
    fallback: &'a str,
) -> &'a str {
    symbols
        .iter()
        .find(|(m, _)| m == mint)
        .map(|(_, s)| *s)
        .unwrap_or(fallback)
}

/// One account row: ✓/✗ · label · short address · lamports (SOL).
fn account_line(
    label: &str,
    exists: bool,
    address: &Pubkey,
    lamports: Option<u64>,
) -> Line<'static> {
    let (mark, mark_color) = if exists {
        ("\u{2713}", Color::Green)
    } else {
        ("\u{2717}", Color::Red)
    };
    let mut spans = vec![
        Span::styled(format!("{mark} "), Style::new().fg(mark_color)),
        Span::raw(format!("{label:<14} ")),
    ];
    if exists {
        spans.push(Span::styled(
            short_pubkey(address),
            Style::new().fg(Color::Gray),
        ));
        if let Some(l) = lamports {
            spans.push(Span::styled(
                format!("  {:.4} SOL", l as f64 / LAMPORTS_PER_SOL as f64),
                Style::new().fg(Color::DarkGray),
            ));
        }
    } else {
        spans.push(Span::styled("—", Style::new().fg(Color::DarkGray)));
    }
    Line::from(spans)
}

/// One participant row: a bullet · label · short address · the holdings in its
/// own wallet, each leg named with its coin ticker and right-aligned in a
/// fixed-width column so the two participant rows line up. Uses a bullet rather
/// than the ✓/✗ of an account row — a participant is an identity that always
/// "is", not an account whose existence is the thing being tracked.
fn participant_line(
    label: &str,
    p: &ParticipantView,
    base_decimals: u8,
    quote_decimals: u8,
    base_symbol: &str,
    quote_symbol: &str,
) -> Line<'static> {
    let base = p.base_tokens as f64 / 10f64.powi(base_decimals as i32);
    let quote = p.quote_tokens as f64 / 10f64.powi(quote_decimals as i32);
    Line::from(vec![
        Span::styled("\u{2022} ", Style::new().fg(Color::DarkGray)),
        Span::raw(format!("{label:<14} ")),
        Span::styled(short_pubkey(&p.address), Style::new().fg(Color::Gray)),
        Span::styled(
            format!("  {base_symbol} {base:>12.2} · {quote_symbol} {quote:>12.2}"),
            Style::new().fg(Color::DarkGray),
        ),
    ])
}

/// `AAAA…oiV`-style abbreviation of a pubkey.
fn short_pubkey(p: &Pubkey) -> String {
    let s = p.to_string();
    if s.len() > 12 {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    } else {
        s
    }
}

/// Render the order-book pane — the reconstructed resting ladder (see
/// [`book`]) for the live market, or a placeholder before one exists.
fn draw_book(f: &mut Frame<'_>, app: &App, area: Rect) {
    let lines = match &app.chain.market {
        Some(market) => book::lines(market),
        None => vec![Line::from(Span::styled(
            "  no market",
            Style::new().fg(Color::DarkGray),
        ))],
    };
    f.render_widget(
        Paragraph::new(lines).block(Block::default().title(" order book ").borders(Borders::ALL)),
        area,
    );
}

/// Render the compute-unit pane: one row per measured operation (in
/// first-seen order), the latest cost for each.
fn draw_cu(f: &mut Frame<'_>, app: &App, area: Rect) {
    let lines: Vec<Line> = if app.cu.is_empty() {
        vec![Line::from(Span::styled(
            "  run an action to measure CU",
            Style::new().fg(Color::DarkGray),
        ))]
    } else {
        app.cu
            .iter()
            .map(|(label, units)| {
                Line::from(vec![
                    Span::raw(format!("{label:<22}")),
                    Span::styled(
                        format!("{:>9} CU", fmt_units(*units)),
                        Style::new().fg(Color::Cyan),
                    ),
                ])
            })
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" compute units ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// Group a CU count into thousands (`41203` → `41,203`) for the pane.
fn fmt_units(n: u64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn draw_log(f: &mut Frame<'_>, app: &App, area: Rect) {
    let height = area.height.saturating_sub(2) as usize;
    let start = app.log.len().saturating_sub(height);
    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .map(|(kind, text)| {
            let color = match kind {
                LogKind::Info => Color::Gray,
                LogKind::Ok => Color::Green,
                LogKind::Err => Color::Red,
            };
            Line::from(Span::styled(text.clone(), Style::new().fg(color)))
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" log → {} ", app.log_path.display()))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Status-bar color for a phase.
fn phase_style(phase: Phase) -> (Color, Modifier) {
    match phase {
        Phase::Ready => (Color::Green, Modifier::BOLD),
        Phase::NoValidator => (Color::Red, Modifier::BOLD),
        _ => (Color::Yellow, Modifier::BOLD),
    }
}

#[cfg(test)]
mod tests {
    use super::{fmt_units, symbol_for};
    use solana_pubkey::Pubkey;

    #[test]
    fn symbol_for_resolves_known_mints_and_falls_back() {
        let base = Pubkey::new_from_array([1u8; 32]);
        let quote = Pubkey::new_from_array([2u8; 32]);
        let other = Pubkey::new_from_array([3u8; 32]);
        let symbols = [(base, "CADC"), (quote, "USDC")];
        assert_eq!(symbol_for(&symbols, &base, "base"), "CADC");
        assert_eq!(symbol_for(&symbols, &quote, "quote"), "USDC");
        // An unknown mint (a market minted outside the bootstrap) falls back.
        assert_eq!(symbol_for(&symbols, &other, "base"), "base");
        assert_eq!(symbol_for(&[], &base, "quote"), "quote");
    }

    #[test]
    fn fmt_units_groups_thousands() {
        assert_eq!(fmt_units(0), "0");
        assert_eq!(fmt_units(999), "999");
        assert_eq!(fmt_units(1_000), "1,000");
        assert_eq!(fmt_units(41_203), "41,203");
        assert_eq!(fmt_units(1_234_567), "1,234,567");
    }
}
