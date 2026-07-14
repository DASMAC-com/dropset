//! Pure rendering of the dashboard from [`App`] state.
//!
//! One screen: a status bar (validator + derived phase + wallet balance), a
//! left action menu (enabled / greyed with reasons, recommended next step
//! marked), a right account table (✓ / ✗ + lamports), and a scrolling log.

use crate::accounts::{ChainState, Liveness, ParticipantView, Phase};
use crate::action::{self, Action};
use crate::app::{swap_side_label, App, LogKind};
use crate::book;
use crate::explorer;
use dropset_sdk::DROPSET_ID;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use std::sync::atomic::Ordering;

/// Number of grouped control rows the "runtime actions" pane renders (bots,
/// swap, eCLOB peg, eCLOB shape, view) — its box height is this plus the
/// top/bottom borders.
const OTHER_ACTIONS_ROWS: u16 = 5;

/// Render the whole dashboard.
pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    // Rebuilt every frame from the current layout: stale rectangles from a
    // prior size/state must not catch clicks.
    app.click_targets.clear();
    app.tx_targets.clear();
    let area = f.area();
    let [status, body, log] = Layout::new(
        Direction::Vertical,
        [
            Constraint::Length(3),
            Constraint::Percentage(60),
            Constraint::Min(6),
        ],
    )
    .areas(area);

    draw_status(f, app, status);

    // Three columns: the action menu (with an alerts pane beneath it), a stacked
    // accounts + CU column, and the markets list stacked above the selected
    // market's order book + fills.
    let [left_area, mid_area, right_area] = Layout::new(
        Direction::Horizontal,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(32),
            Constraint::Percentage(38),
        ],
    )
    .areas(body);
    // The left column stacks the phase-action menu (its entries + borders) over
    // the grouped "other actions" pane (a fixed number of control rows +
    // borders), with alerts taking the rest of the column below them.
    let menu_height = (action::MENU.len() as u16 + 2).clamp(3, 14);
    let [menu_area, other_area, alerts_area] = Layout::new(
        Direction::Vertical,
        [
            Constraint::Length(menu_height),
            Constraint::Length(OTHER_ACTIONS_ROWS + 2),
            Constraint::Min(3),
        ],
    )
    .areas(left_area);
    draw_menu(f, app, menu_area);
    draw_other_actions(f, app, other_area);
    draw_alerts(f, app, alerts_area);
    let [accounts_area, cu_area] = Layout::new(
        Direction::Vertical,
        [Constraint::Percentage(62), Constraint::Percentage(38)],
    )
    .areas(mid_area);
    draw_accounts(f, app, accounts_area);
    draw_cu(f, app, cu_area);
    // The markets list is as tall as its rows (plus borders), capped so the
    // book + fills keep most of the column; they take the rest.
    let markets_height = (app.chain.markets.len() as u16 + 2).clamp(3, 12);
    let [markets_area, lower_area] = Layout::new(
        Direction::Vertical,
        [Constraint::Length(markets_height), Constraint::Min(8)],
    )
    .areas(right_area);
    draw_markets(f, app, markets_area);
    // The book (top) sits over a recent-fills tape (bottom), mirroring the
    // accounts / CU split in the middle column.
    let [book_area, fills_area] = Layout::new(
        Direction::Vertical,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .areas(lower_area);
    draw_book(f, app, book_area);
    draw_fills(f, app, fills_area);

    draw_log(f, app, log);
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
            "  ·  admin {:.3} SOL",
            app.chain.wallet_lamports as f64 / LAMPORTS_PER_SOL as f64
        )),
        Span::raw("  ·  bots "),
        bots_status(app),
        Span::raw("  ·  takers "),
        takers_status(app),
        Span::raw("  ·  swap "),
        Span::styled(
            format!(
                "{} units {}",
                app.swap_units,
                swap_side_label(app.swap_side)
            ),
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
        .block(
            Block::default()
                .title(" actions · setup ")
                .borders(Borders::ALL),
        )
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

/// Render the "runtime actions" pane beneath the setup menu: the letter-key
/// controls that aren't part of the numbered bootstrap lifecycle — bot toggles,
/// the swap (with its current amount and side), the eCLOB peg / reshape
/// controls (each annotated with its step size), and the view keys. This pane
/// is the single home for these runtime controls; the footer carries only menu
/// navigation, so nothing here is duplicated there. Grouped by kind with a dim
/// tag. The market-scoped groups (swap, eCLOB) dim when no live vault is
/// selected, mirroring how the setup menu greys steps that can't run yet. Keep
/// the line count in sync with [`OTHER_ACTIONS_ROWS`], which sizes the box.
fn draw_other_actions(f: &mut Frame<'_>, app: &App, area: Rect) {
    let ready = app.chain.phase() == Phase::Ready;
    let tag = |s: &'static str| Span::styled(format!("{s:<6}"), Style::new().fg(Color::DarkGray));
    let live = Style::new().fg(Color::Gray);
    // Market-scoped controls read live only once a seeded vault exists.
    let market_style = if ready {
        live
    } else {
        Style::new().fg(Color::DarkGray)
    };

    // The swap row doubles as the amount input: while the taker is typing a new
    // amount (`a`), it echoes the digits with a block cursor instead of the
    // static control hint, so the input happens here in the runtime pane rather
    // than hijacking the footer.
    let swap_line = match &app.amount_input {
        Some(buf) => Line::from(vec![
            tag("swap"),
            Span::styled(
                format!("amount: {buf}\u{2588}  Enter ok · Esc cancel"),
                Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]),
        None => Line::from(vec![
            tag("swap"),
            Span::styled(
                format!(
                    "s swap · S flip · a amount   [{}u {}]",
                    app.swap_units,
                    swap_side_label(app.swap_side)
                ),
                market_style,
            ),
        ]),
    };
    // eCLOB controls split across two rows so each keeps its step-size
    // annotation (±bps) without overflowing the narrow left column: the peg /
    // spread nudges on one row, the shape presets (thin, reset one / all) on
    // the next.
    let lines = vec![
        Line::from(vec![
            tag("bots"),
            Span::styled("m/M maker · t/T taker · x stop all", live),
        ]),
        swap_line,
        Line::from(vec![
            tag("peg"),
            Span::styled(
                "< > re-peg \u{00b1}5 bps · w/n spread \u{00b1}5 bps",
                market_style,
            ),
        ]),
        Line::from(vec![
            tag("shape"),
            Span::styled("f thin far side · g reset · G reset all", market_style),
        ]),
        Line::from(vec![tag("view"), Span::styled("r refresh · q quit", live)]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" actions · runtime ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// Render the alerts pane beneath the actions — environment / config
/// conditions the operator should know about (Docker missing, an unset feed
/// key, a degraded FX feed), each a colored bullet. Shows a green "all clear"
/// line when nothing is wrong, so the pane always reads as a live health check.
fn draw_alerts(f: &mut Frame<'_>, app: &App, area: Rect) {
    let mut alerts: Vec<(Color, String)> = Vec::new();

    // The managed explorer container / Docker.
    match app.ctx.explorer_state.load(Ordering::SeqCst) {
        explorer::state::NO_DOCKER => alerts.push((
            Color::Yellow,
            "Docker not found — explorer falls back to the hosted site, which \
             may not reach the localnet in Brave/Safari."
                .to_string(),
        )),
        explorer::state::FAILED => alerts.push((
            Color::Red,
            "Explorer container failed to start — see the log.".to_string(),
        )),
        _ => {}
    }

    // The maker's optional CoinMarketCap secondary feed key.
    if std::env::var("CMC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .is_none()
    {
        alerts.push((
            Color::DarkGray,
            "CMC_API_KEY unset — maker FX feed uses CoinGecko → FX-rate → static.".to_string(),
        ));
    }

    // The live FX feed, as reported by the maker's streamed log lines.
    if app.feed_degraded {
        alerts.push((
            Color::Yellow,
            "FX feed unavailable (rate-limited?) — maker quoting on the fallback peg.".to_string(),
        ));
    }

    let lines: Vec<Line> = if alerts.is_empty() {
        vec![Line::from(Span::styled(
            "\u{2713} all clear",
            Style::new().fg(Color::Green),
        ))]
    } else {
        alerts
            .into_iter()
            .map(|(color, msg)| {
                Line::from(Span::styled(
                    format!("\u{2022} {msg}"),
                    Style::new().fg(color),
                ))
            })
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(" alerts ").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
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
        Paragraph::new(lines).block(
            Block::default()
                .title(" accounts · click to open ")
                .borders(Borders::ALL),
        ),
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
        // Underlined to read as a hyperlink — the whole row is a click target
        // that opens the account in the explorer (see [`draw_accounts`]).
        spans.push(Span::styled(
            short_pubkey(address),
            Style::new()
                .fg(Color::Gray)
                .add_modifier(Modifier::UNDERLINED),
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
        // Underlined like the account rows — a clickable link to the explorer.
        Span::styled(
            short_pubkey(&p.address),
            Style::new()
                .fg(Color::Gray)
                .add_modifier(Modifier::UNDERLINED),
        ),
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
/// status: a ✓/○ liquidity glyph, its flag + ticker, the book mid (or "idle"),
/// a green ● when its maker bot is running, and a cyan ● when its taker is. The
/// selected market is marked and bold; `[` / `]` move the selection, which
/// drives the order book below.
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
        Paragraph::new(lines).block(
            Block::default()
                .title(" markets · [ ] to select ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// One markets-list row for market `i`: selection marker · liquidity glyph ·
/// flag · ticker · book mid (or "idle") · the leader's reference price (the fair
/// value the maker pegs to) · a green ● when its maker is running · a cyan ●
/// when its taker is.
fn market_row(app: &App, i: usize, market: &crate::accounts::MarketView) -> Line<'static> {
    let symbol = symbol_for(&app.mint_symbols, &market.base_mint, "?");
    let flag = flag_for(symbol);
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
    let price = mid.map_or_else(|| "idle".to_string(), book::fmt_price);
    // The stamped reference (fair value) the maker pegs to, distinct from the
    // reconstructed book mid — a dash before any vault has quoted one.
    let reference = market
        .reference_price
        .map_or_else(|| "\u{2014}".to_string(), book::fmt_price);

    let mut spans = vec![
        Span::raw(marker.to_string()),
        Span::styled(format!("{glyph} "), Style::new().fg(glyph_color)),
        Span::raw(format!("{flag} ")),
        Span::styled(format!("{symbol:<6}"), symbol_style),
        // Label both prices inline so the columns are self-describing: the book
        // mid and the leader's stamped reference (fair value).
        Span::styled("  mid ", Style::new().fg(Color::DarkGray)),
        Span::styled(format!("{price:>11}"), Style::new().fg(Color::Gray)),
        Span::styled("  ref ", Style::new().fg(Color::DarkGray)),
        Span::styled(format!("{reference:>11}"), Style::new().fg(Color::DarkGray)),
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
            let flag = flag_for(symbol);
            (
                format!(" order book · {flag} {symbol} "),
                book::lines(market),
            )
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

/// Render the recent-fills tape for the selected market — a column header, then
/// newest-first rows (time · buy/sell · price · size · tx link). Fed by the
/// `emit_cpi!` `FillEvent` subscription ([`crate::fills`]); empty until a swap
/// lands on this market. Each row is a click target that opens its swap in the
/// explorer. Degenerate zero-size / zero-value fills are skipped.
fn draw_fills(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    // Snapshot the selected market's info as owned values so the fills / target
    // borrows below don't overlap the chain borrow.
    let Some((address, base_dec, quote_dec, symbol)) =
        app.chain.selected_market(app.selected_market).map(|m| {
            (
                m.address,
                m.base_decimals,
                m.quote_decimals,
                symbol_for(&app.mint_symbols, &m.base_mint, "market").to_string(),
            )
        })
    else {
        f.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                "  no market",
                Style::new().fg(Color::DarkGray),
            ))])
            .block(
                Block::default()
                    .title(" recent fills ")
                    .borders(Borders::ALL),
            ),
            area,
        );
        return;
    };
    // Rows that fit below the header, newest first — snapshot to owned data so
    // the fills read-borrow ends before the target-registration write-borrow.
    let height = (area.height.saturating_sub(2) as usize).saturating_sub(1);
    let rows: Vec<(String, String, u8, f64, f64, f64)> = app
        .fills
        .iter()
        .rev()
        .filter(|r| r.event.market == address && r.event.fill_base > 0 && r.event.fill_quote > 0)
        .take(height)
        .map(|r| {
            let size = r.event.fill_base as f64 / 10f64.powi(base_dec as i32);
            let value = r.event.fill_quote as f64 / 10f64.powi(quote_dec as i32);
            (
                r.time.clone(),
                r.signature.clone(),
                r.event.side,
                value / size,
                size,
                value,
            )
        })
        .collect();

    let flag = flag_for(&symbol);
    let title = format!(" recent fills · {flag} {symbol} ");
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                "  no fills yet — run a swap or the taker",
                Style::new().fg(Color::DarkGray),
            ))])
            .block(Block::default().title(title).borders(Borders::ALL)),
            area,
        );
        return;
    }

    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let inner_w = area.width.saturating_sub(2);
    let max_y = area.y + area.height.saturating_sub(1);
    let mut lines = vec![fills_header()];
    for (i, (time, sig, side, price, size, volume)) in rows.iter().enumerate() {
        // Row 0 is the header, so data rows start one below it.
        let y = inner_y + 1 + i as u16;
        if !sig.is_empty() && y < max_y {
            app.tx_targets.push((
                Rect {
                    x: inner_x,
                    y,
                    width: inner_w,
                    height: 1,
                },
                sig.clone(),
            ));
        }
        lines.push(fill_line(time, *side, *price, *size, *volume, sig));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

/// The fills-tape column header, styled like the order book's — `time · side ·
/// price · size · volume · txn`, widths matching [`fill_line`].
fn fills_header() -> Line<'static> {
    let style = Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(format!("{:>8}", "time"), style),
        Span::styled(format!("  {:<4}", "side"), style),
        Span::styled(format!("  {:>10}", "price"), style),
        Span::styled(format!(" {:>8}", "size"), style),
        Span::styled(format!(" {:>8}", "volume"), style),
        Span::styled(format!("  {}", "txn"), style),
    ])
}

/// One fills-tape row: the observed time, a colored buy/sell tag (green buy, red
/// sell — the taker's aggressor side), the fill price (quote per base, adaptive
/// precision so small-value tokens keep their significant figures), the base
/// size, the quote-denominated volume (price × size), and an underlined short
/// signature linking to the swap in the explorer.
fn fill_line(time: &str, side: u8, price: f64, size: f64, volume: f64, sig: &str) -> Line<'static> {
    // side: 0 = taker Buy (lifts the ask), 1 = taker Sell (hits the bid).
    let (label, color) = if side == 0 {
        ("buy ", Color::Rgb(30, 135, 80))
    } else {
        ("sell", Color::Rgb(240, 75, 90))
    };
    Line::from(vec![
        Span::styled(format!("{time:>8}"), Style::new().fg(Color::DarkGray)),
        Span::styled(
            format!("  {label:<4}"),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {:>10}", book::fmt_price(price)),
            Style::new().fg(color),
        ),
        Span::styled(format!(" {size:>8.2}"), Style::new().fg(Color::DarkGray)),
        Span::styled(format!(" {volume:>8.2}"), Style::new().fg(Color::DarkGray)),
        // Leading gap kept out of the underline (see [`cu_line`]).
        Span::raw("  "),
        Span::styled(
            short_sig(sig),
            Style::new()
                .fg(Color::Gray)
                .add_modifier(Modifier::UNDERLINED),
        ),
    ])
}

/// A country / region flag emoji for a known token symbol — the fiat it tracks
/// — so the markets list and book read like the frontend's flagged rows. Empty
/// for a symbol outside the bootstrap roster (no known flag to show).
fn flag_for(symbol: &str) -> &'static str {
    match symbol {
        "EURC" => "\u{1F1EA}\u{1F1FA}", // 🇪🇺
        "VCHF" => "\u{1F1E8}\u{1F1ED}", // 🇨🇭
        "TGBP" => "\u{1F1EC}\u{1F1E7}", // 🇬🇧
        "ZARP" => "\u{1F1FF}\u{1F1E6}", // 🇿🇦
        "MXNe" => "\u{1F1F2}\u{1F1FD}", // 🇲🇽
        "XSGD" => "\u{1F1F8}\u{1F1EC}", // 🇸🇬
        "IDRX" => "\u{1F1EE}\u{1F1E9}", // 🇮🇩
        "USDC" => "\u{1F1FA}\u{1F1F8}", // 🇺🇸
        _ => "",
    }
}

/// Render the compute-unit pane: a column header, then one row per measured
/// operation (in first-seen order), each with its latest cost, the time it was
/// recorded, and a link to the transaction that measured it. Registers each row
/// as a click target (like the accounts pane) so a left-click opens that
/// operation's latest tx in the explorer — re-running an operation (e.g. a
/// repeg) updates the cost, the time, and the linked tx.
fn draw_cu(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    if app.cu.is_empty() {
        f.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                "  run an action to measure CU",
                Style::new().fg(Color::DarkGray),
            ))])
            .block(
                Block::default()
                    .title(" compute units ")
                    .borders(Borders::ALL),
            ),
            area,
        );
        return;
    }
    // Snapshot the rows so the target-registration borrow of `app.tx_targets`
    // doesn't overlap the read borrow of `app.cu` (cheap — a handful of rows).
    let rows: Vec<(String, u64, String, String)> = app
        .cu
        .iter()
        .map(|r| {
            (
                r.label.clone(),
                r.units,
                r.time.clone(),
                r.signature.clone(),
            )
        })
        .collect();
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let inner_w = area.width.saturating_sub(2);
    let max_y = area.y + area.height.saturating_sub(1);
    let mut lines = vec![cu_header()];
    for (i, (label, units, time, sig)) in rows.iter().enumerate() {
        // Row 0 is the header, so data rows start one below it.
        let y = inner_y + 1 + i as u16;
        if !sig.is_empty() && y < max_y {
            app.tx_targets.push((
                Rect {
                    x: inner_x,
                    y,
                    width: inner_w,
                    height: 1,
                },
                sig.clone(),
            ));
        }
        lines.push(cu_line(label, *units, time, sig));
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" compute units · click tx ")
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// The CU-pane column header, styled like the order book's — `op · cu · time ·
/// tx`, widths matching [`cu_line`].
fn cu_header() -> Line<'static> {
    let style = Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(format!("{:<22}", "op"), style),
        Span::styled(format!("{:>10}", "cu"), style),
        Span::styled(format!("  {:>8}", "time"), style),
        Span::styled(format!("  {}", "txn"), style),
    ])
}

/// One CU-pane row: the operation label, its latest measured cost, the time it
/// was recorded, and an underlined short signature linking to the transaction.
/// The label column is wide enough for the longest op (`set_liquidity_profile`)
/// so the numeric columns stay aligned.
fn cu_line(label: &str, units: u64, time: &str, sig: &str) -> Line<'static> {
    let mut spans = vec![
        Span::raw(format!("{label:<22}")),
        Span::styled(
            format!("{:>10}", fmt_units(units)),
            Style::new().fg(Color::Cyan),
        ),
        Span::styled(format!("  {time:>8}"), Style::new().fg(Color::DarkGray)),
    ];
    if !sig.is_empty() {
        // Keep the leading gap out of the underlined span, or the underline
        // reads as a stray "__" before the signature.
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            short_sig(sig),
            Style::new()
                .fg(Color::Gray)
                .add_modifier(Modifier::UNDERLINED),
        ));
    }
    Line::from(spans)
}

/// `AAAAAA…oiV`-style abbreviation of a base58 transaction signature (ASCII, so
/// byte slicing is safe), for the txn link columns.
fn short_sig(s: &str) -> String {
    if s.len() > 10 {
        format!("{}\u{2026}{}", &s[..4], &s[s.len() - 4..])
    } else {
        s.to_string()
    }
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
    use super::{flag_for, fmt_units, liveness_color, symbol_for};
    use crate::accounts::Liveness;
    use ratatui::style::Color;
    use solana_pubkey::Pubkey;

    #[test]
    fn flag_for_maps_known_symbols_and_falls_back() {
        // Each bootstrap token resolves to its fiat's flag emoji…
        assert_eq!(flag_for("EURC"), "\u{1F1EA}\u{1F1FA}");
        assert_eq!(flag_for("IDRX"), "\u{1F1EE}\u{1F1E9}");
        assert_eq!(flag_for("USDC"), "\u{1F1FA}\u{1F1F8}");
        // …and an unknown symbol shows no flag rather than a wrong one.
        assert_eq!(flag_for("????"), "");
    }

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
