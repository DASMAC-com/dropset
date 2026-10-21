//! Pure rendering of the dashboard from [`App`] state.
//!
//! One screen: a status bar (validator + derived phase + wallet balance), a
//! left action menu (enabled / greyed with reasons, recommended next step
//! marked), a right account table (✓ / ✗ + lamports), and a scrolling log.

use crate::accounts::{self, ChainState, Liveness, MarketView, ParticipantView, Phase, VaultSeat};
use crate::action::{self, Action};
use crate::app::{
    parse_positive_amount, swap_side_label, App, LeaderPrompt, LogKind, PassivePrompt,
};
use crate::book;
use crate::cluster::Cluster;
use crate::explorer;
use crate::leader::{self, LeaderOp, Leg};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

/// Number of grouped control rows the "runtime actions" pane renders (bots,
/// swap, eCLOB peg, eCLOB shape, passive ladder, vault, view — mainnet's six
/// leave one blank) — its box height is this plus the top/bottom borders.
const RUNTIME_ACTIONS_ROWS: u16 = 7;

/// Inner rows of the leader pane: address, status, wallet, vault, stake.
const LEADER_PANE_ROWS: u16 = 5;
/// Inner rows of the swapper pane: address, wallet.
const SWAPPER_PANE_ROWS: u16 = 2;

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
    let menu_height = (app.menu_entries().len() as u16 + 2).clamp(3, 14);
    let [menu_area, other_area, alerts_area] = Layout::new(
        Direction::Vertical,
        [
            Constraint::Length(menu_height),
            Constraint::Length(RUNTIME_ACTIONS_ROWS + 2),
            Constraint::Min(3),
        ],
    )
    .areas(left_area);
    draw_menu(f, app, menu_area);
    draw_runtime_actions(f, app, other_area);
    draw_alerts(f, app, alerts_area);
    // The middle column stacks the program-side accounts, then the market's
    // two participants in their own labelled panes, over the CU tape, which
    // takes the rest.
    let account_rows = build_account_rows(&app.chain, app.ctx.cluster, app.selected_market);
    let [accounts_area, leader_area, swapper_area, cu_area] = Layout::new(
        Direction::Vertical,
        [
            Constraint::Length(account_rows.len() as u16 + 2),
            Constraint::Length(LEADER_PANE_ROWS + 2),
            Constraint::Length(SWAPPER_PANE_ROWS + 2),
            Constraint::Min(5),
        ],
    )
    .areas(mid_area);
    draw_accounts(f, app, account_rows, accounts_area);
    draw_leader_pane(f, app, leader_area);
    draw_swapper_pane(f, app, swapper_area);
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
    // Last, so it paints over the dashboard rather than under it.
    draw_leader_prompt(f, app, area);
    draw_passive_prompt(f, app, area);
    draw_ceremony_prompt(f, app, area);
}

/// The passive-ladder prompt, centered over the dashboard while it is open:
/// the parameter form, then the previewed ladder with the typed-`yes` line.
/// Bordered like the leader prompt, so mainnet names itself at the moment of
/// sending.
fn draw_passive_prompt(f: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(prompt) = &app.passive_prompt else {
        return;
    };
    let input = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
    let hint = Style::new().fg(Color::DarkGray);
    let warn = Style::new().fg(Color::Yellow);
    let (label, body, entry, keys) = match prompt {
        // The form, then the vault as it opened: what flipped since the last
        // stamp is what the daily visit looks at before re-centering.
        PassivePrompt::Form(form) => (
            form.op.label(),
            form.lines()
                .into_iter()
                .map(|l| Line::from(Span::styled(l, input)))
                .chain((!form.vault.is_empty()).then(|| Line::from("")))
                .chain(form.vault.iter().map(|l| Line::from(l.clone())))
                .collect::<Vec<_>>(),
            "Enter preview".to_string(),
            "\u{2191}\u{2193} field · \u{2190}\u{2192} step · digits type the price · Esc cancel",
        ),
        PassivePrompt::Confirm { ticket, buf } => (
            ticket.op.label(),
            ticket
                .summary()
                .into_iter()
                .map(|l| {
                    if l.starts_with("warning:") {
                        Line::from(Span::styled(l, warn))
                    } else {
                        Line::from(l)
                    }
                })
                .collect(),
            format!("Type 'yes' to sign and send: {buf}\u{2588}"),
            "Enter ok · Esc cancel",
        ),
    };
    // The entry line leads: an eight-rung preview runs past twenty rows, and
    // a short terminal clips the bottom of the box, which must never be the
    // line the operator types `yes` into.
    let mut lines = vec![
        Line::from(Span::styled(entry, input)),
        Line::from(Span::styled(keys, hint)),
        Line::from(""),
    ];
    lines.extend(body);
    let (title, border) = if app.ctx.cluster.is_mainnet() {
        (
            format!(" {label} — MAINNET · REAL FUNDS "),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            format!(" {label} — localnet "),
            Style::new().fg(Color::Yellow),
        )
    };
    let width = area.width.min(80);
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border),
        ),
        rect,
    );
}

/// The mainnet ceremony prompt, centered over the dashboard while it is open:
/// the step's plan — every address and amount it would write, read fresh —
/// over the typed-`yes` line, in the same red real-funds border as the leader
/// prompt.
fn draw_ceremony_prompt(f: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(prompt) = &app.ceremony_prompt else {
        return;
    };
    let input = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
    let hint = Style::new().fg(Color::DarkGray);
    let mut lines: Vec<Line> = prompt.plan.iter().map(|l| Line::from(l.as_str())).collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("Type 'yes' to sign and send: {}\u{2588}", prompt.buf),
        input,
    )));
    lines.push(Line::from(Span::styled("Enter ok · Esc cancel", hint)));
    let width = area.width.min(100);
    // Sized by wrapped rows, not lines: a plan line carrying two addresses
    // wraps, and counting it as one would clip the `yes` line off the bottom.
    let inner = usize::from(width.saturating_sub(2)).max(1);
    let rows: usize = lines.iter().map(|l| l.width().div_ceil(inner).max(1)).sum();
    let height = u16::try_from(rows + 2).unwrap_or(u16::MAX).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(format!(
                    " {} — {} · REAL FUNDS ",
                    prompt.action.label(),
                    app.ctx.cluster.label().to_uppercase()
                ))
                .borders(Borders::ALL)
                .border_style(Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ),
        rect,
    );
}

/// The leader deposit / withdraw prompt, centered over the dashboard while it
/// is open: the amount entry, then the sized ticket with the typed-`yes` line.
/// Mainnet says so in the border — the same words as the launch banner — so
/// the one box the operator is reading at the moment of sending names the
/// chain.
fn draw_leader_prompt(f: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(prompt) = &app.leader_prompt else {
        return;
    };
    let input = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
    let hint = Style::new().fg(Color::DarkGray);
    let (op, lines) = match prompt {
        LeaderPrompt::Amount { op, buf } => (*op, amount_stage_lines(app, *op, buf)),
        LeaderPrompt::Confirm { ticket, buf } => {
            let mut lines: Vec<Line> = ticket.summary().into_iter().map(Line::from).collect();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Type 'yes' to send: {buf}\u{2588}"),
                input,
            )));
            lines.push(Line::from(Span::styled("Enter ok · Esc cancel", hint)));
            (ticket.op, lines)
        }
    };
    let title = if app.ctx.cluster.is_mainnet() {
        format!(" {} — MAINNET · REAL FUNDS ", op.label())
    } else {
        format!(" {} — localnet ", op.label())
    };
    let border = if app.ctx.cluster.is_mainnet() {
        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::Yellow)
    };
    let width = area.width.min(72);
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border),
        ),
        rect,
    );
}

/// The amount stage of the leader prompt: what the leader holds now, the
/// typed amount, and a live preview of what it sizes to — all against the
/// last poll, so typing costs no RPC. Enter still sizes from a fresh read,
/// and the confirm stage shows that ticket, not this preview.
fn amount_stage_lines(app: &App, op: LeaderOp, buf: &str) -> Vec<Line<'static>> {
    let input = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
    let hint = Style::new().fg(Color::DarkGray);
    let seat = leader_seat(app);
    let mut lines: Vec<Line> = match seat {
        Some((mkt, seat)) => {
            let (b, q) = market_legs(app, mkt);
            leader::holding_lines(seat.seq, &seat.stake, b, q, mkt.reference_price)
                .into_iter()
                .map(|(label, value)| field(label, Span::raw(value)))
                .collect()
        }
        None => vec![Line::from(Span::styled(
            "no single led vault in the last poll — Enter reads the chain",
            hint,
        ))],
    };
    lines.push(Line::from(Span::styled(
        format!("amount, in {}:", op.amount_hint()),
        hint,
    )));
    lines.push(Line::from(Span::styled(format!("{buf}\u{2588}"), input)));
    let preview = seat.and_then(|(mkt, seat)| {
        let (b, q) = market_legs(app, mkt);
        // The same parse Enter applies, so the preview never sizes what
        // Enter would refuse to.
        leader::preview(
            op,
            &seat.stake,
            parse_positive_amount(buf),
            b,
            q,
            mkt.reference_price,
        )
    });
    match preview {
        Some(Ok(rows)) => lines.extend(rows.into_iter().map(Line::from)),
        Some(Err(reason)) => lines.push(Line::from(Span::styled(
            format!("preview  {reason}"),
            Style::new().fg(Color::Red),
        ))),
        None => {}
    }
    lines.push(Line::from(Span::styled(
        "Enter size from a fresh read · Esc cancel",
        hint,
    )));
    lines
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
    let cluster = app.ctx.cluster;
    let next = action::recommended_next(phase, cluster);
    let items: Vec<ListItem> = app
        .menu_entries()
        .iter()
        .enumerate()
        .map(|(i, &a)| menu_item(i, a, phase, cluster, next))
        .collect();
    // Name the cluster in the border on mainnet, so the one pane the operator
    // is always looking at says which chain this is. The localnet title is
    // unchanged — every existing session reads the same as before.
    let title = if app.ctx.cluster.is_mainnet() {
        " actions · MAINNET ".to_string()
    } else {
        " actions · setup ".to_string()
    };
    let list = List::new(items)
        .block(Block::default().title(title).borders(Borders::ALL))
        .highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_stateful_widget(list, area, &mut app.menu);
}

fn menu_item(
    i: usize,
    action: Action,
    phase: Phase,
    cluster: Cluster,
    next: Option<Action>,
) -> ListItem<'static> {
    let enabled = action.enabled(phase, cluster);
    let recommended = next == Some(action);
    // Only `1..=9` are keys; an entry past them is reached by `j`/`k` + Enter,
    // and a "10." would name a key that does not exist.
    let key_label = if i < action::NUMBER_KEYS {
        format!("{}. ", i + 1)
    } else {
        "   ".to_string()
    };
    let key = Span::styled(key_label, Style::new().fg(Color::DarkGray));
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
            format!("  ({})", action.disabled_reason(phase, cluster)),
            Style::new().fg(Color::DarkGray),
        ));
    }
    ListItem::new(Line::from(spans))
}

/// Render the "runtime actions" pane beneath the setup menu: the letter-key
/// controls that aren't part of the numbered bootstrap lifecycle — bot toggles,
/// the swap (with its current amount and side), the eCLOB peg / reshape
/// controls (each annotated with its step size), and the view keys. This pane
/// is the single on-screen home for these runtime controls. Grouped by kind
/// with a dim tag. The market-scoped groups (swap, eCLOB) dim when no live
/// vault is selected, mirroring how the setup menu greys steps that can't run
/// yet. Keep the line count in sync with [`RUNTIME_ACTIONS_ROWS`], which sizes
/// the box.
///
/// On mainnet the bot, swap and eCLOB controls are refused — the bots and
/// the eCLOB controls sign with committed localnet role keys, the swap with
/// the committed taker — so the pane says so instead of advertising keys that
/// only produce an error line, and advertises the passive ladder and the kill
/// switch instead. The `vault` row is live on both: the leader's stake
/// commands are confirm-gated writes mainnet keeps. Same row count, so the
/// layout does not shift.
fn draw_runtime_actions(f: &mut Frame<'_>, app: &App, area: Rect) {
    let tag = |s: &'static str| Span::styled(format!("{s:<6}"), Style::new().fg(Color::DarkGray));
    let live = Style::new().fg(Color::Gray);
    let off = Style::new().fg(Color::DarkGray);
    let ready = app.chain.phase() == Phase::Ready;
    // The leader's stake: live once every vault is seeded (the commands'
    // own gate), and the reserved rotation always greyed.
    let vault_line = Line::from(vec![
        tag("vault"),
        Span::styled("+ deposit · - withdraw", if ready { live } else { off }),
        Span::styled(" · rotate (reserved)", off),
    ]);
    if app.ctx.cluster.is_mainnet() {
        let lines = vec![
            Line::from(vec![
                tag("bots"),
                Span::styled("localnet only (committed role keys)", off),
            ]),
            Line::from(vec![
                tag("swap"),
                Span::styled("localnet only (no mainnet taker key)", off),
            ]),
            // The demo controls are localnet only; the passive ladder is the
            // mainnet book, deployed and re-centered from here.
            Line::from(vec![
                tag("ladder"),
                Span::styled("p passive ladder · c re-center", live),
            ]),
            vault_line,
            // The kill switch, in the real-funds red the prompts use. Close
            // and drain follow it from the menu.
            Line::from(vec![
                tag("kill"),
                Span::styled(
                    "K kill ladders · then menu: close, drain",
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![tag("view"), Span::styled("r refresh · q quit", live)]),
        ];
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .title(" actions · runtime · MAINNET ")
                    .borders(Borders::ALL),
            ),
            area,
        );
        return;
    }
    // Market-scoped controls read live only once a seeded vault exists.
    let market_style = if ready { live } else { off };

    // The swap row doubles as the amount input: while the taker is typing a new
    // amount (`a`), it echoes the digits with a block cursor instead of the
    // static control hint, so the input happens inline on this row rather than
    // in a separate prompt.
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
        Line::from(vec![
            tag("ladder"),
            Span::styled("p passive ladder · c re-center · P posture", market_style),
        ]),
        vault_line,
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

/// Render a set of market symbols as one comma-separated alert subject, so a
/// condition affecting several markets is one line rather than one line each.
fn joined(symbols: &BTreeSet<String>) -> String {
    symbols.iter().cloned().collect::<Vec<_>>().join(", ")
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

    // A suspected wiring error is pushed ahead of the standing advisory that
    // follows it. The pane groups by subject rather than sorting by severity,
    // so this orders the pair against each other, not against the bullets above.
    if !app.basis_config_suspects.is_empty() {
        alerts.push((
            Color::Red,
            format!(
                "{}: first basis reading was out of band — check the feed \
                 config, not the peg.",
                joined(&app.basis_config_suspects)
            ),
        ));
    }

    // Markets with no independent basis source. A deliberate fourth color: not
    // Yellow, because nothing is wrong and nothing will recover, so it must not
    // read as a degrade the operator is waiting out; and not the DarkGray used
    // for inert notes like an unset API key, because this one qualifies every
    // price the market quotes and should stay legible.
    if !app.unverified_markets.is_empty() {
        alerts.push((
            Color::Cyan,
            format!(
                "{}: no independent basis source — quoting the FX anchor on a \
                 pinned basis (unverified).",
                joined(&app.unverified_markets)
            ),
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

/// Register inner row `i` of the bordered pane at `area` as a click target
/// opening `addr` in the explorer — a no-op for a row past the bottom border,
/// which is not drawn and so not clickable. The inner area sits one cell in
/// from the border on every side.
fn click_row(app: &mut App, area: Rect, i: usize, addr: Pubkey) {
    let y = area.y + 1 + i as u16;
    if y >= area.y + area.height.saturating_sub(1) {
        return;
    }
    app.click_targets.push((
        Rect {
            x: area.x + 1,
            y,
            width: area.width.saturating_sub(2),
            height: 1,
        },
        addr,
    ));
}

fn draw_accounts(
    f: &mut Frame<'_>,
    app: &mut App,
    rows: Vec<(Line<'static>, Option<Pubkey>)>,
    area: Rect,
) {
    // A left-click anywhere on an address-bearing row opens that account.
    for (i, (_, addr)) in rows.iter().enumerate() {
        if let Some(addr) = addr {
            click_row(app, area, i, *addr);
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
/// address in the explorer. `selected` picks which discovered market's
/// accounts to show. The market's participants are not here: they have their
/// own panes ([`draw_leader_pane`], [`draw_swapper_pane`]).
fn build_account_rows(
    chain: &ChainState,
    cluster: Cluster,
    selected: usize,
) -> Vec<(Line<'static>, Option<Pubkey>)> {
    // The cluster's program id is fixed and meaningful even before deploy, so
    // its row is always clickable; absent registry/market rows have no real
    // address.
    let program = cluster.program_id();
    let mut rows: Vec<(Line<'static>, Option<Pubkey>)> = vec![(
        account_line("program", chain.program_deployed, &program, None),
        Some(program),
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

/// The selected market's two legs, each `(ticker, decimals)`, named by the
/// known-mint map so the participant panes show actual coins.
fn market_legs<'a>(app: &'a App, mkt: &MarketView) -> (Leg<'a>, Leg<'a>) {
    (
        (
            symbol_for(&app.mint_symbols, &mkt.base_mint, "base"),
            mkt.base_decimals,
        ),
        (
            symbol_for(&app.mint_symbols, &mkt.quote_mint, "quote"),
            mkt.quote_decimals,
        ),
    )
}

/// The roster leader's one vault on the selected market, as of the last
/// poll — `None` without a leader key, a market, or exactly one led vault.
fn leader_seat(app: &App) -> Option<(&MarketView, VaultSeat)> {
    let mkt = app.chain.selected_market(app.selected_market)?;
    let leader = app.chain.roster_leader?;
    Some((mkt, accounts::sole_seat_led_by(&mkt.vaults, &leader)?))
}

/// One labelled field row: a dim fixed-width label, then the value.
fn field(label: &str, value: Span<'static>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), Style::new().fg(Color::DarkGray)),
        value,
    ])
}

/// A participant's address as an underlined, clickable field — the row the
/// pane registers as its explorer link.
fn address_field(address: &Pubkey) -> Line<'static> {
    field(
        "address",
        Span::styled(
            short_pubkey(address),
            Style::new()
                .fg(Color::Gray)
                .add_modifier(Modifier::UNDERLINED),
        ),
    )
}

/// The token holdings in a participant's own wallet, each leg named.
fn wallet_field(p: &ParticipantView, (bs, bd): Leg, (qs, qd): Leg) -> Line<'static> {
    let base = p.base_tokens as f64 / 10f64.powi(i32::from(bd));
    let quote = p.quote_tokens as f64 / 10f64.powi(i32::from(qd));
    field(
        "wallet",
        Span::raw(format!("{base:.2} {bs} · {quote:.2} {qs}")),
    )
}

/// What [`Liveness`] means for the leader's bot, in words beside its color.
fn liveness_word(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Live => "quoting",
        Liveness::Stale => "stale",
        Liveness::Unknown => "no signal",
    }
}

/// The participant's address when its pane will draw it — both it and a
/// selected market are known — so a pane showing "not read yet" registers
/// no click target.
fn drawn_address(app: &App, p: Option<&ParticipantView>) -> Option<Pubkey> {
    app.chain.selected_market(app.selected_market)?;
    p.map(|p| p.address)
}

/// A bordered pane titled `title`, drawn with `lines`.
fn draw_pane(f: &mut Frame<'_>, title: &str, lines: Vec<Line<'static>>, area: Rect) {
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(" {title} "))
                .borders(Borders::ALL),
        ),
        area,
    );
}

/// The vault leader (the MM bot) on the selected market: its address (the
/// explorer link), whether its bot is quoting, its own wallet, and its stake
/// in the vault it leads — shares, share of the vault, floor, and its slice
/// of the inventory valued at the market's reference price.
fn draw_leader_pane(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    if let Some(address) = drawn_address(app, app.chain.leader.as_ref()) {
        click_row(app, area, 0, address);
    }
    let app: &App = app;
    let (Some(p), Some(mkt)) = (
        &app.chain.leader,
        app.chain.selected_market(app.selected_market),
    ) else {
        return draw_pane(f, "leader", vec![count_line("not read yet".into())], area);
    };
    let (b, q) = market_legs(app, mkt);
    let mut lines = vec![
        address_field(&p.address),
        field(
            "status",
            Span::styled(
                liveness_word(p.liveness),
                Style::new().fg(liveness_color(p.liveness)),
            ),
        ),
        wallet_field(p, b, q),
    ];
    match leader_seat(app) {
        Some((_, seat)) => lines.extend(
            leader::holding_lines(seat.seq, &seat.stake, b, q, mkt.reference_price)
                .into_iter()
                .map(|(label, value)| field(label, Span::raw(value))),
        ),
        None => lines.push(field(
            "vault",
            // `leader_seat` is `None` for no led vault, several, or no
            // leader key alike.
            Span::styled(
                "not exactly one led on this market",
                Style::new().fg(Color::DarkGray),
            ),
        )),
    }
    draw_pane(f, "leader", lines, area);
}

/// The swapper (the taker role key) on the selected market: its address (the
/// explorer link) and its own wallet.
fn draw_swapper_pane(f: &mut Frame<'_>, app: &mut App, area: Rect) {
    if let Some(address) = drawn_address(app, app.chain.swapper.as_ref()) {
        click_row(app, area, 0, address);
    }
    let app: &App = app;
    let (Some(p), Some(mkt)) = (
        &app.chain.swapper,
        app.chain.selected_market(app.selected_market),
    ) else {
        return draw_pane(f, "swapper", vec![count_line("not read yet".into())], area);
    };
    let (b, q) = market_legs(app, mkt);
    let lines = vec![address_field(&p.address), wallet_field(p, b, q)];
    draw_pane(f, "swapper", lines, area);
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
/// value the maker pegs to) · `passive` for a passive-posture market · a green
/// ● when its maker is running · a cyan ● when its taker is.
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
    // Passive markets run with no maker, so say so rather than leave the
    // missing ● to read as a maker the operator forgot to start.
    if app.postures.of(symbol) == crate::market::Posture::Passive {
        spans.push(Span::styled("  passive", Style::new().fg(Color::Yellow)));
    }
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
        "AUDD" => "\u{1F1E6}\u{1F1FA}", // 🇦🇺
        "CADC" => "\u{1F1E8}\u{1F1E6}", // 🇨🇦
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

    /// Every bootstrapped market must have a flag, because this table is a
    /// hand-maintained duplicate of the roster and nothing else connects them.
    ///
    /// The failure it catches is silent by construction: an unknown symbol
    /// falls through to `""`, which is also what a market with no flag would
    /// legitimately render, so a roster addition loses its flag and the list
    /// just looks slightly wrong. Measured — AUDD and CADC shipped flagless
    /// and it took an operator noticing two blank cells in the markets list.
    ///
    /// The quote mint is checked too: it is not in `PAIRS` as a base, but the
    /// book and fills panes flag it.
    #[test]
    fn every_bootstrapped_market_has_a_flag() {
        for pair in crate::market::PAIRS {
            assert!(
                !flag_for(pair.base.symbol).is_empty(),
                "{} is bootstrapped but has no flag in `flag_for`",
                pair.base.symbol
            );
        }
        assert!(!flag_for("USDC").is_empty(), "the quote mint needs one too");
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
