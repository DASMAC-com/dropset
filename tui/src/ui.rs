//! Pure rendering of the dashboard from [`App`] state.
//!
//! One screen: a status bar (validator + derived phase + wallet balance), a
//! left action menu (enabled / greyed with reasons, recommended next step
//! marked), a right account table (✓ / ✗ + lamports), and a scrolling log.

use crate::accounts::{ChainState, Liveness, ParticipantView, Phase};
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
            Constraint::Length(4),
        ],
    )
    .areas(area);

    draw_status(f, app, status);

    // Three columns: the action menu, a stacked accounts + CU column, and the
    // markets list stacked above the selected market's order book.
    let [menu_area, mid_area, right_area] = Layout::new(
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
    // The markets list is as tall as its rows (plus borders), capped so the
    // book keeps most of the column; the book takes the rest.
    let markets_height = (app.chain.markets.len() as u16 + 2).clamp(3, 12);
    let [markets_area, book_area] = Layout::new(
        Direction::Vertical,
        [Constraint::Length(markets_height), Constraint::Min(6)],
    )
    .areas(right_area);
    draw_markets(f, app, markets_area);
    draw_book(f, app, book_area);

    draw_log(f, app, log);

    // While the taker is typing a swap amount, the footer becomes the input
    // prompt; otherwise it shows the keybind help.
    match &app.amount_input {
        Some(buf) => draw_amount_prompt(f, buf, footer),
        None => draw_help(f, footer),
    }
}

/// Render the keybind-help footer.
fn draw_help(f: &mut Frame<'_>, area: Rect) {
    let help = Paragraph::new(vec![
        Line::from(
            "j/k menu  ·  enter/1-9 run  ·  [ ] market  ·  s maker  ·  S all  ·  \
             T taker  ·  x stop all  ·  a swap amount  ·  r refresh  ·  q quit",
        ),
        Line::from(
            "eCLOB · selected market:  < > re-peg \u{00b1}5 bps  ·  w widen  ·  \
             t tighten  ·  f thin far side  ·  g reset ladder",
        ),
    ])
    .block(Block::default().borders(Borders::ALL))
    .alignment(Alignment::Center);
    f.render_widget(help, area);
}

/// Render the swap-amount input prompt in place of the help footer, echoing the
/// digits typed so far with a block cursor.
fn draw_amount_prompt(f: &mut Frame<'_>, buf: &str, area: Rect) {
    let prompt = Line::from(vec![
        Span::styled(
            "swap amount (quote units): ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{buf}\u{2588}"), Style::new().fg(Color::White)),
    ]);
    let hint = Line::from(Span::styled(
        "type digits  ·  Enter confirm  ·  Backspace delete  ·  Esc cancel",
        Style::new().fg(Color::DarkGray),
    ));
    f.render_widget(
        Paragraph::new(vec![prompt, hint])
            .block(Block::default().borders(Borders::ALL))
            .alignment(Alignment::Center),
        area,
    );
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
        Span::raw("  ·  bots "),
        bots_status(app),
        Span::raw("  ·  takers "),
        takers_status(app),
        Span::raw("  ·  swap "),
        Span::styled(
            format!("{} units", app.swap_quote_units),
            Style::new().fg(Color::Cyan),
        ),
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

/// Status-bar span for the maker bots: how many are running out of the
/// discovered markets, green once any is up.
fn bots_status(app: &App) -> Span<'static> {
    let running = app.bots.running_count();
    let total = app.chain.markets.len();
    let color = if running > 0 {
        Color::Green
    } else {
        Color::DarkGray
    };
    Span::styled(format!("{running}/{total}"), Style::new().fg(color))
}

/// Status-bar span for the taker bots (opt-in flow): how many are running out
/// of the discovered markets, green once any is up — a running-count so the
/// operator sees at a glance whether any book is being driven with flow.
fn takers_status(app: &App) -> Span<'static> {
    let running = app.takers.running_count();
    let total = app.chain.markets.len();
    // Cyan when any taker is up — matching the cyan taker dot in the markets
    // list (the maker is green in both places, the taker cyan in both).
    let color = if running > 0 {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    Span::styled(format!("{running}/{total}"), Style::new().fg(color))
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
    let rows = build_account_rows(&app.chain, &app.mint_symbols, app.selected_market);
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
/// `selected` picks which discovered market's accounts to show.
fn build_account_rows(
    chain: &ChainState,
    symbols: &[(Pubkey, &'static str)],
    selected: usize,
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

    match chain.selected_market(selected) {
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

/// One participant row: a status dot · label · short address · the holdings in
/// its own wallet, each leg named with its coin ticker and right-aligned in a
/// fixed-width column so the two participant rows line up. Uses a bullet rather
/// than the ✓/✗ of an account row — a participant is an identity that always
/// "is", not an account whose existence is the thing being tracked — but colors
/// that bullet by [`Liveness`]: green when the bot is quoting, yellow once its
/// quotes have aged, dim when the TUI has no signal to observe (see
/// [`liveness_color`]).
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
        Span::styled("\u{2022} ", Style::new().fg(liveness_color(p.liveness))),
        Span::raw(format!("{label:<14} ")),
        Span::styled(short_pubkey(&p.address), Style::new().fg(Color::Gray)),
        Span::styled(
            format!("  {base_symbol} {base:>12.2} · {quote_symbol} {quote:>12.2}"),
            Style::new().fg(Color::DarkGray),
        ),
    ])
}

/// The status-dot color for a participant's liveness: green when quoting,
/// yellow once its quotes have aged, and the dim gray of an unobserved
/// participant otherwise — so a running bot's gray dot turns green, and a
/// stopped one fades to yellow before going gray when its vault is gone.
fn liveness_color(liveness: Liveness) -> Color {
    match liveness {
        Liveness::Live => Color::Green,
        Liveness::Stale => Color::Yellow,
        Liveness::Unknown => Color::DarkGray,
    }
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

/// Render the markets list — every discovered market with its per-market
/// status: a ✓/○ liquidity glyph, its ticker, the book mid (or "idle"), a green
/// ● when its maker bot is running, and a cyan ● when its taker is. The selected
/// market is marked and bold; `[` / `]` move the selection, which drives the
/// order book below.
fn draw_markets(f: &mut Frame<'_>, app: &App, area: Rect) {
    let lines: Vec<Line> = if app.chain.markets.is_empty() {
        vec![Line::from(Span::styled(
            "  no markets — bootstrap first",
            Style::new().fg(Color::DarkGray),
        ))]
    } else {
        app.chain
            .markets
            .iter()
            .enumerate()
            .map(|(i, m)| market_row(app, i, m))
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines).block(Block::default().title(" markets ").borders(Borders::ALL)),
        area,
    );
}

/// One markets-list row for market `i`: selection marker · liquidity glyph ·
/// ticker · book mid (or "idle") · a green ● when its maker is running · a cyan
/// ● when its taker is.
fn market_row(app: &App, i: usize, market: &crate::accounts::MarketView) -> Line<'static> {
    let symbol = symbol_for(&app.mint_symbols, &market.base_mint, "?");
    let selected = i == app.selected_market;
    let mid = book::mid_price(market);

    // ✓ (green) when the book has resting liquidity, ○ (gray) when idle.
    let (glyph, glyph_color) = if mid.is_some() {
        ("\u{2713}", Color::Green)
    } else {
        ("\u{25cb}", Color::DarkGray)
    };
    let marker = if selected { "> " } else { "  " };
    let symbol_style = if selected {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let price = mid.map_or_else(|| "idle".to_string(), |p| format!("{p:.4}"));

    let mut spans = vec![
        Span::raw(marker.to_string()),
        Span::styled(format!("{glyph} "), Style::new().fg(glyph_color)),
        Span::styled(format!("{symbol:<6}"), symbol_style),
        Span::styled(format!(" {price:>10}"), Style::new().fg(Color::Gray)),
    ];
    if app.bots.is_running(symbol) {
        spans.push(Span::styled(
            "  \u{25cf} maker",
            Style::new().fg(Color::Green),
        ));
    }
    if app.takers.is_running(symbol) {
        spans.push(Span::styled(
            "  \u{25cf} taker",
            Style::new().fg(Color::Cyan),
        ));
    }
    Line::from(spans)
}

/// Render the order-book pane — the reconstructed resting ladder (see
/// [`book`]) for the selected market, or a placeholder before one exists. The
/// pane title carries the selected market's ticker.
fn draw_book(f: &mut Frame<'_>, app: &App, area: Rect) {
    let (title, lines) = match app.chain.selected_market(app.selected_market) {
        Some(market) => {
            let symbol = symbol_for(&app.mint_symbols, &market.base_mint, "market");
            (format!(" order book · {symbol} "), book::lines(market))
        }
        None => (
            " order book ".to_string(),
            vec![Line::from(Span::styled(
                "  no market",
                Style::new().fg(Color::DarkGray),
            ))],
        ),
    };
    f.render_widget(
        Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
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
    use super::{fmt_units, liveness_color, symbol_for};
    use crate::accounts::Liveness;
    use ratatui::style::Color;
    use solana_pubkey::Pubkey;

    #[test]
    fn liveness_color_maps_each_state() {
        assert_eq!(liveness_color(Liveness::Live), Color::Green);
        assert_eq!(liveness_color(Liveness::Stale), Color::Yellow);
        assert_eq!(liveness_color(Liveness::Unknown), Color::DarkGray);
    }

    #[test]
    fn symbol_for_resolves_known_mints_and_falls_back() {
        let base = Pubkey::new_from_array([1u8; 32]);
        let quote = Pubkey::new_from_array([2u8; 32]);
        let other = Pubkey::new_from_array([3u8; 32]);
        let symbols = [(base, "EURC"), (quote, "USDC")];
        assert_eq!(symbol_for(&symbols, &base, "base"), "EURC");
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
