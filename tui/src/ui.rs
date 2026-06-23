//! Pure rendering of the dashboard from [`App`] state.
//!
//! One screen: a status bar (validator + derived phase + wallet balance), a
//! left action menu (enabled / greyed with reasons, recommended next step
//! marked), a right account table (✓ / ✗ + lamports), and a scrolling log.

use crate::accounts::Phase;
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

    let help = Paragraph::new("j/k move  ·  enter / 1-9 run  ·  r refresh  ·  q quit")
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

fn draw_accounts(f: &mut Frame<'_>, app: &App, area: Rect) {
    let mut lines = vec![account_line(
        "program",
        app.chain.program_deployed,
        &DROPSET_ID,
        None,
    )];

    match &app.chain.registry {
        Some(reg) => {
            lines.push(account_line(
                "registry",
                true,
                &reg.address,
                Some(reg.lamports),
            ));
            lines.push(account_line(
                "fee vault",
                true,
                &reg.fee_vault,
                Some(reg.fee_vault_lamports),
            ));
            lines.push(Line::from(Span::styled(
                format!("  markets: {}", reg.market_count),
                Style::new().fg(Color::DarkGray),
            )));
        }
        None => lines.push(account_line("registry", false, &Pubkey::default(), None)),
    }

    match &app.chain.market {
        Some(mkt) => {
            lines.push(account_line(
                "market",
                true,
                &mkt.address,
                Some(mkt.lamports),
            ));
            lines.push(account_line(
                "base treasury",
                true,
                &mkt.base_treasury,
                Some(mkt.base_treasury_lamports),
            ));
            lines.push(account_line(
                "quote treasury",
                true,
                &mkt.quote_treasury,
                Some(mkt.quote_treasury_lamports),
            ));
            lines.push(Line::from(Span::styled(
                format!("  active vaults: {}", mkt.active_count),
                Style::new().fg(Color::DarkGray),
            )));
        }
        None => lines.push(account_line("market", false, &Pubkey::default(), None)),
    }

    f.render_widget(
        Paragraph::new(lines).block(Block::default().title(" accounts ").borders(Borders::ALL)),
        area,
    );
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

/// Render the compute-unit pane: one row per measured operation (newest
/// last), the latest cost for each.
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
