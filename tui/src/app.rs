//! Application state and the synchronous event loop.
//!
//! The loop mirrors the `anchor debugger`'s: draw, then service input with a
//! burst-drain so a held key collapses into one render. On top of that it
//! also drains background-job events and re-polls on-chain state on a timer
//! (or immediately when a job signals a change), so the panel tracks the
//! validator without a blocking read. A [`TerminalGuard`] restores the
//! terminal on the way out; the owned [`Validator`] kills the child and
//! wipes its temp ledger on `Drop`, so quitting leaves no orphan.

// cspell:word Deque
// cspell:word pasteable
// cspell:word RAII

use crate::accounts::{self, ChainState, Liveness};
use crate::action::{self, Action, JobContext};
use crate::bot::{self, BotManager};
use crate::chain;
use crate::explorer;
use crate::fills;
use crate::job::{JobEvent, Logger};
use crate::market;
use crate::ui;
use crate::validator::Validator;
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use dropset_sdk::matching::SwapSide;
use dropset_sdk::types::FillEvent;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Position, Rect},
    widgets::ListState,
    Terminal,
};
use solana_client::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Number of entries in the action menu.
const MENU_LEN: usize = action::MENU.len();
/// Re-poll on-chain state at least this often, even with no job activity.
const REFRESH_INTERVAL: Duration = Duration::from_millis(700);
/// How many log lines to retain.
const LOG_CAPACITY: usize = 1_000;
/// How many recent fills to retain across all markets — the fills pane shows a
/// per-market slice of this ring, newest first.
const FILLS_CAPACITY: usize = 500;
/// Longest swap-amount the taker can type, in digits — bounded so the entered
/// value always parses as a `u64` (its max is 20 digits).
const MAX_AMOUNT_DIGITS: usize = 18;
/// eCLOB widen / tighten step, in bps of bid-ask spread — `w` adds it, `n`
/// subtracts it, rebuilding the ladder at the new spread.
const SPREAD_STEP_BPS: u32 = 5;
/// Clamp on the manually-stepped spread so it stays a sane, encodable book.
const MIN_SPREAD_BPS: u32 = 5;
const MAX_SPREAD_BPS: u32 = 1_000;

/// Kind of a log line — drives its color.
#[derive(Clone, Copy)]
pub enum LogKind {
    Info,
    Ok,
    Err,
}

/// One compute-units row: the operation label, its latest measured cost, the
/// signature of the transaction that produced it (the explorer link), and when
/// it was recorded (`HH:MM:SS`).
pub(crate) struct CuRow {
    pub(crate) label: String,
    pub(crate) units: u64,
    pub(crate) signature: String,
    pub(crate) time: String,
}

/// One recent fill for the fills pane: when the TUI observed it (`HH:MM:SS`),
/// the signature of the swap that produced it (the explorer link), and the
/// decoded event.
pub(crate) struct FillRow {
    pub(crate) time: String,
    pub(crate) signature: String,
    pub(crate) event: FillEvent,
}

/// Whether the loop should keep running.
#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

/// The whole TUI.
pub struct App {
    pub(crate) ctx: JobContext,
    pub(crate) validator: Validator,
    pub(crate) client: RpcClient,
    pub(crate) chain: ChainState,
    pub(crate) menu: ListState,
    pub(crate) log: VecDeque<(LogKind, String)>,
    pub(crate) job_running: bool,
    /// Measured compute-unit cost per operation, in first-seen order — one row
    /// per `label`, updated in place when an operation runs again. Each row also
    /// carries the signature and timestamp of the latest transaction for that
    /// operation, which the CU pane links to the explorer (so re-running e.g. a
    /// repeg refreshes the link to the newest `set_reference_price` tx). Drives
    /// the CU pane.
    pub(crate) cu: Vec<CuRow>,
    /// Path the log is mirrored to on disk (shown in the log pane title).
    pub(crate) log_path: PathBuf,
    /// The swapper / taker (`FFFF`) pubkey, resolved once at startup, so each
    /// poll can read its holdings for the accounts pane. `None` if the role
    /// key can't be loaded.
    swapper: Option<Pubkey>,
    /// Known mint → ticker, resolved once at startup from the bootstrap's
    /// fixed mint keys, so the accounts pane can name a discovered
    /// market's coins (the chain scan only yields mint pubkeys).
    pub(crate) mint_symbols: Vec<(Pubkey, &'static str)>,
    /// Index into [`ChainState::markets`] of the market whose order book and
    /// accounts are shown — the "which bot's book am I looking at" selection,
    /// moved with `[` / `]`.
    pub(crate) selected_market: usize,
    /// Whole units of the input token a probe swap spends — quote units on a
    /// Buy, base units on a Sell — editable by the taker via the amount input
    /// (`a`). Seeded from [`action::DEFAULT_PROBE_UNITS`].
    pub(crate) swap_units: u64,
    /// Which side a probe swap (`s`) takes: `Buy` pays quote and receives base,
    /// `Sell` pays base and receives quote. Flipped with `S`; defaults to `Buy`.
    pub(crate) swap_side: SwapSide,
    /// When set, the taker is typing a new swap amount — the digits entered so
    /// far. While it is `Some`, keystrokes edit this buffer and the normal
    /// keybinds are suppressed, so a digit types a number rather than firing a
    /// menu action; Enter commits it to `swap_units`, Esc cancels.
    pub(crate) amount_input: Option<String>,
    /// The per-market maker-bot child processes the operator starts and stops.
    pub(crate) bots: BotManager,
    /// The per-market taker-bot child processes — opt-in, off by default. The
    /// operator flips one on to give the selected book organic flow during a
    /// walkthrough; keyed by symbol like [`App::bots`], so a market's maker and
    /// taker are tracked independently.
    pub(crate) takers: BotManager,
    /// Address click targets for the accounts pane, rebuilt by [`ui::draw`]
    /// each frame: a left-click inside one of these rectangles opens that
    /// account in the explorer.
    pub(crate) click_targets: Vec<(Rect, Pubkey)>,
    /// Transaction click targets for the CU and fills panes, rebuilt by
    /// [`ui::draw`] each frame: a left-click inside one of these rectangles
    /// opens that transaction in the explorer.
    pub(crate) tx_targets: Vec<(Rect, String)>,
    /// Recent decoded fills across every market (newest at the back), fed by
    /// the [`fills`] subscription thread. The fills pane renders the slice for
    /// the selected market; the ring is trimmed to [`FILLS_CAPACITY`].
    pub(crate) fills: VecDeque<FillRow>,
    tx: Sender<JobEvent>,
    rx: Receiver<JobEvent>,
    /// Append handle for the on-disk log mirror, if it opened.
    log_file: Option<File>,
    last_refresh: Instant,
    dirty: bool,
    /// Whether the maker bot's FX feed is currently reporting itself down —
    /// derived from its streamed `[feed] coingecko …` log lines, surfaced as an
    /// alert. Set on a failure line, cleared on a recovery line (or a wipe).
    pub(crate) feed_degraded: bool,
    /// The current eCLOB manual bid-ask spread (bps) the widen / tighten
    /// controls step. Seeded at the default; `w` / `n` step it by
    /// [`SPREAD_STEP_BPS`] and rebuild the ladder at the new spread.
    spread_bps: u32,
}

impl App {
    /// Spawn the validator and build the app. `ctx.rpc_url` is overwritten
    /// with the spawned validator's URL.
    pub fn new(mut ctx: JobContext) -> Result<Self> {
        let validator = Validator::spawn()?;
        ctx.rpc_url = validator.rpc_url().to_string();
        let client = chain::rpc(validator.rpc_url());
        let (tx, rx) = mpsc::channel();
        let mut menu = ListState::default();
        menu.select(Some(0));
        // Mirror the log to a fresh file so the full (untruncated) output —
        // RPC errors included — survives outside the scrollback.
        let log_path = std::env::temp_dir().join("dropset-tui.log");
        let log_file = File::create(&log_path).ok();
        // Resolve the swapper (FFFF) once — used to read its holdings each poll.
        let swapper = market::taker(&ctx.repo_root).ok().map(|k| k.pubkey());
        // Resolve the known mint tickers once — used to label the accounts pane.
        let mint_symbols = market::mint_symbols(&ctx.repo_root);
        Ok(Self {
            ctx,
            validator,
            client,
            chain: ChainState::default(),
            menu,
            log: VecDeque::new(),
            job_running: false,
            cu: Vec::new(),
            log_path,
            swapper,
            mint_symbols,
            selected_market: 0,
            swap_units: action::DEFAULT_PROBE_UNITS,
            swap_side: SwapSide::Buy,
            amount_input: None,
            bots: BotManager::new(),
            takers: BotManager::new(),
            click_targets: Vec::new(),
            tx_targets: Vec::new(),
            fills: VecDeque::new(),
            tx,
            rx,
            log_file,
            // Force an immediate first poll.
            last_refresh: Instant::now() - REFRESH_INTERVAL,
            dirty: true,
            feed_degraded: false,
            spread_bps: market::DEFAULT_SPREAD_BPS,
        })
    }

    /// Run the event loop until the user quits.
    pub fn run(&mut self) -> Result<()> {
        self.log(LogKind::Info, "Starting solana-test-validator…".to_string());
        self.start_explorer();
        // Watch the program's fills so the recent-fills pane fills in as swaps
        // land — survives a wipe by reconnecting to the respawned validator.
        fills::spawn(self.ctx.rpc_url.clone(), self.tx.clone());
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        let mut guard = TerminalGuard::new(terminal);

        loop {
            guard.term.draw(|f| ui::draw(f, self))?;

            if event::poll(Duration::from_millis(100))? {
                // Block for the first event, then drain the burst crossterm
                // buffered (held j/k) so it collapses into one render.
                let mut flow = self.handle_event(event::read()?);
                let mut drained = 0;
                while flow == Flow::Continue && drained < 256 && event::poll(Duration::ZERO)? {
                    flow = self.handle_event(event::read()?);
                    drained += 1;
                }
                if flow == Flow::Quit {
                    break;
                }
            }

            self.drain_jobs();
            self.maybe_refresh();
        }
        Ok(())
    }

    /// Bring the local explorer container up on a background thread, so it is
    /// serving by the time the operator opens it. Streams its output into the
    /// log without occupying the single-job slot, so bootstrapping stays
    /// available while the (first-time) image builds.
    fn start_explorer(&self) {
        let log = Logger::new(self.tx.clone());
        let repo_root = self.ctx.repo_root.clone();
        let status = self.ctx.explorer_state.clone();
        let lock = self.ctx.explorer_lock.clone();
        std::thread::spawn(move || {
            explorer::start_in_background(&log, &repo_root, &status, &lock);
        });
    }

    fn handle_event(&mut self, ev: Event) -> Flow {
        match ev {
            Event::Key(k) => self.handle_key(k),
            Event::Mouse(m) => {
                self.handle_mouse(m);
                Flow::Continue
            }
            _ => Flow::Continue,
        }
    }

    /// Handle a mouse event: a left-click on an account row opens that account
    /// in the explorer; a click on a CU row opens that operation's latest
    /// transaction in the explorer; everything else is ignored.
    fn handle_mouse(&mut self, m: MouseEvent) {
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }
        if let Some(addr) = self.address_at(m.column, m.row) {
            self.open_in_explorer(addr);
        } else if let Some(sig) = self.signature_at(m.column, m.row) {
            self.open_tx_in_explorer(&sig);
        }
    }

    /// The account address whose click-target rect contains `(col, row)`, if
    /// any.
    fn address_at(&self, col: u16, row: u16) -> Option<Pubkey> {
        hit_target(&self.click_targets, col, row)
    }

    /// The transaction signature whose CU click-target rect contains
    /// `(col, row)`, if any.
    fn signature_at(&self, col: u16, row: u16) -> Option<String> {
        hit_target(&self.tx_targets, col, row)
    }

    /// Whether the local explorer is serving, logging a hint before a hosted
    /// fallback so a click that lands on explorer.solana.com isn't a silent
    /// surprise. The hint only fires while the container is still coming up or
    /// after it failed — the two cases where the local explorer *would* have
    /// served; a no-Docker host has no local explorer to wait on, so it falls
    /// back quietly.
    fn explorer_ready(&mut self) -> bool {
        match self.ctx.explorer_state.load(Ordering::SeqCst) {
            explorer::state::READY => true,
            explorer::state::STARTING => {
                self.log(
                    LogKind::Info,
                    "Local explorer not ready yet (still starting) — \
                     opening the hosted explorer, which may not reach localnet."
                        .to_string(),
                );
                false
            }
            explorer::state::FAILED => {
                self.log(
                    LogKind::Info,
                    "Local explorer failed to start — opening the hosted \
                     explorer, which may not reach localnet."
                        .to_string(),
                );
                false
            }
            _ => false,
        }
    }

    /// Open `address` in the explorer — the local container when it's ready,
    /// else the hosted explorer as a fallback (which can't reach the localnet
    /// in some browsers; see [`explorer`]). Best-effort: a launch failure is
    /// logged, not fatal.
    fn open_in_explorer(&mut self, address: Pubkey) {
        let url = if self.explorer_ready() {
            explorer::account_url(&address, &self.ctx.rpc_url)
        } else {
            explorer::hosted_account_url(&address, &self.ctx.rpc_url)
        };
        self.log(LogKind::Info, format!("Opening {address} in the explorer…"));
        if let Err(e) = open::that(&url) {
            self.log(LogKind::Err, format!("open explorer: {e:#}"));
        }
    }

    /// Open transaction `signature` in the explorer — the local container when
    /// ready, else the hosted explorer (same browser caveat as
    /// [`App::open_in_explorer`]). Best-effort: a launch failure is logged.
    fn open_tx_in_explorer(&mut self, signature: &str) {
        let url = if self.explorer_ready() {
            explorer::tx_url(signature, &self.ctx.rpc_url)
        } else {
            explorer::hosted_tx_url(signature, &self.ctx.rpc_url)
        };
        self.log(
            LogKind::Info,
            format!("Opening tx {signature} in the explorer…"),
        );
        if let Err(e) = open::that(&url) {
            self.log(LogKind::Err, format!("open explorer: {e:#}"));
        }
    }

    fn handle_key(&mut self, k: KeyEvent) -> Flow {
        // While entering a swap amount, keystrokes edit the buffer instead of
        // driving the dashboard — this must come before the normal keybinds so
        // a digit types a number (and Esc cancels rather than quitting).
        if self.amount_input.is_some() {
            self.handle_amount_key(k);
            return Flow::Continue;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Flow::Quit,
            KeyCode::Char('c') if ctrl => return Flow::Quit,
            KeyCode::Char('j') | KeyCode::Down => self.menu_step(1),
            KeyCode::Char('k') | KeyCode::Up => self.menu_step(-1),
            // Book selector — cycle which market's book and accounts show.
            KeyCode::Char('[') | KeyCode::BackTab => self.select_market(-1),
            KeyCode::Char(']') | KeyCode::Tab => self.select_market(1),
            // Per-bot control, makers and takers symmetric: lower-case toggles
            // the selected market's bot, upper-case toggles every market's (start
            // all if none are running, else stop them all). The taker is opt-in
            // (off by default), so `t` / `T` only ever start or stop flow, never
            // quote. Makers live on `m` / `M` (not `s` / `S`) so `s` is free for
            // the swap; eCLOB tighten is on `n` (narrow) to free `t`.
            KeyCode::Char('m') => self.toggle_selected_bot(),
            KeyCode::Char('M') => self.toggle_all_bots(),
            KeyCode::Char('t') => self.toggle_selected_taker(),
            KeyCode::Char('T') => self.toggle_all_takers(),
            KeyCode::Char('x') => self.stop_all_bots(),
            KeyCode::Char('r') => self.dirty = true,
            // Swap on the selected market: `s` dispatches the probe at the
            // current amount / side, `S` flips the side (Buy ⇄ Sell).
            KeyCode::Char('s') => self.run_action(Action::ProbeSwap),
            KeyCode::Char('S') => self.flip_swap_side(),
            // Open the swap-amount input — subsequent keys edit the amount.
            KeyCode::Char('a') => self.begin_amount_input(),
            // eCLOB demo (selected market): reprice the anchor (whole-book
            // shift) vs reshape the ladder (shape change at a fixed peg).
            KeyCode::Char('>') | KeyCode::Char('.') => self.run_action(Action::RepegUp),
            KeyCode::Char('<') | KeyCode::Char(',') => self.run_action(Action::RepegDown),
            KeyCode::Char('w') => self.step_spread(Action::WidenSpread),
            KeyCode::Char('n') => self.step_spread(Action::TightenSpread),
            KeyCode::Char('f') => self.run_action(Action::ThinFarSide),
            KeyCode::Char('g') => {
                self.spread_bps = market::DEFAULT_SPREAD_BPS;
                self.run_action(Action::ResetLadder);
            }
            // Broadcast reset: return every market's ladder to the default
            // shape at once (re-arm the whole demo after nudging several books).
            KeyCode::Char('G') => {
                self.spread_bps = market::DEFAULT_SPREAD_BPS;
                self.run_action(Action::ResetAllLadders);
            }
            KeyCode::Enter => self.run_selected(),
            KeyCode::Char(d @ '1'..='8') => {
                let idx = (d as usize) - ('1' as usize);
                if idx < MENU_LEN {
                    self.menu.select(Some(idx));
                    self.run_selected();
                }
            }
            _ => {}
        }
        Flow::Continue
    }

    /// Move the market selection by `delta`, wrapping across the discovered
    /// markets. A no-op until any market exists. Forces a re-poll so the
    /// accounts pane's participant holdings track the newly selected market.
    fn select_market(&mut self, delta: isize) {
        let n = self.chain.markets.len();
        if n == 0 {
            return;
        }
        let cur = self.selected_market.min(n - 1) as isize;
        self.selected_market = (cur + delta).rem_euclid(n as isize) as usize;
        self.dirty = true;
    }

    /// The ticker of the currently selected market, from its base mint via the
    /// known-mint map — `None` before any market exists, or for a market minted
    /// outside the bootstrap (no known symbol, so no bot to drive).
    fn selected_symbol(&self) -> Option<&'static str> {
        let market = self.chain.selected_market(self.selected_market)?;
        self.mint_symbols
            .iter()
            .find(|(m, _)| *m == market.base_mint)
            .map(|(_, s)| *s)
    }

    /// Toggle the selected market's maker bot — start it if stopped (flash
    /// liquidity), stop it if running.
    fn toggle_selected_bot(&mut self) {
        let Some(symbol) = self.selected_symbol() else {
            self.log(
                LogKind::Err,
                "No market selected to drive a bot.".to_string(),
            );
            return;
        };
        if self.bots.is_running(symbol) {
            self.bots.stop(symbol);
            self.log(LogKind::Ok, format!("[{symbol}] bot stopped"));
        } else {
            let log = Logger::new(self.tx.clone());
            let cmd = bot::maker_command(&self.ctx.repo_root, symbol, &self.ctx.rpc_url);
            if let Err(e) = self.bots.start(symbol, cmd, &log) {
                self.log(LogKind::Err, format!("[{symbol}] start failed: {e:#}"));
            }
        }
        self.dirty = true;
    }

    /// Toggle the selected market's taker bot — start it against that exact
    /// market to give the book organic flow, or stop it to leave the market
    /// quiet. Off by default: nothing runs a taker until the operator flips it
    /// on here. Needs the selected market's address (the taker is scoped by
    /// PDA), so it is a no-op before a market exists.
    fn toggle_selected_taker(&mut self) {
        let Some(symbol) = self.selected_symbol() else {
            self.log(
                LogKind::Err,
                "No market selected to drive a taker.".to_string(),
            );
            return;
        };
        if self.takers.is_running(symbol) {
            self.takers.stop(symbol);
            self.log(LogKind::Ok, format!("[{symbol}] taker stopped"));
        } else {
            // Unreachable in practice: `selected_symbol()` above already
            // resolved from this same selected market, so it exists — the guard
            // is just defensive against a race the single-threaded UI can't hit.
            let Some(address) = self
                .chain
                .selected_market(self.selected_market)
                .map(|m| m.address)
            else {
                return;
            };
            let log = Logger::new(self.tx.clone());
            let cmd = bot::taker_command(&self.ctx.repo_root, &address, &self.ctx.rpc_url);
            if let Err(e) = self.takers.start(symbol, cmd, &log) {
                self.log(
                    LogKind::Err,
                    format!("[{symbol}] taker start failed: {e:#}"),
                );
            }
        }
        self.dirty = true;
    }

    /// Toggle every market's maker bot at once (the "flash liquidity across the
    /// board" control): if any maker is running, stop them all; otherwise start
    /// one per discovered market that isn't already running.
    fn toggle_all_bots(&mut self) {
        if self.bots.running_count() > 0 {
            let n = self.bots.running_count();
            self.bots.stop_all();
            self.log(LogKind::Ok, format!("Stopped {n} maker bot(s)."));
            self.dirty = true;
            return;
        }
        let symbols: Vec<&'static str> = self
            .chain
            .markets
            .iter()
            .filter_map(|m| {
                self.mint_symbols
                    .iter()
                    .find(|(mint, _)| *mint == m.base_mint)
                    .map(|(_, s)| *s)
            })
            .collect();
        let log = Logger::new(self.tx.clone());
        for symbol in symbols {
            if !self.bots.is_running(symbol) {
                let cmd = bot::maker_command(&self.ctx.repo_root, symbol, &self.ctx.rpc_url);
                if let Err(e) = self.bots.start(symbol, cmd, &log) {
                    self.log(LogKind::Err, format!("[{symbol}] start failed: {e:#}"));
                }
            }
        }
        self.dirty = true;
    }

    /// Toggle every market's taker bot at once, mirroring [`toggle_all_bots`]:
    /// if any taker is running, stop them all; otherwise start one per
    /// discovered market (each scoped to its book by PDA). Opt-in like the
    /// per-market taker — nothing runs a taker until the operator presses `T`.
    fn toggle_all_takers(&mut self) {
        if self.takers.running_count() > 0 {
            let n = self.takers.running_count();
            self.takers.stop_all();
            self.log(LogKind::Ok, format!("Stopped {n} taker bot(s)."));
            self.dirty = true;
            return;
        }
        let targets: Vec<(&'static str, Pubkey)> = self
            .chain
            .markets
            .iter()
            .filter_map(|m| {
                self.mint_symbols
                    .iter()
                    .find(|(mint, _)| *mint == m.base_mint)
                    .map(|(_, s)| (*s, m.address))
            })
            .collect();
        let log = Logger::new(self.tx.clone());
        for (symbol, address) in targets {
            if !self.takers.is_running(symbol) {
                let cmd = bot::taker_command(&self.ctx.repo_root, &address, &self.ctx.rpc_url);
                if let Err(e) = self.takers.start(symbol, cmd, &log) {
                    self.log(
                        LogKind::Err,
                        format!("[{symbol}] taker start failed: {e:#}"),
                    );
                }
            }
        }
        self.dirty = true;
    }

    /// Stop every running bot — makers and takers alike, so `x` quiets the
    /// whole demo in one keystroke.
    fn stop_all_bots(&mut self) {
        let n = self.bots.running_count() + self.takers.running_count();
        self.bots.stop_all();
        self.takers.stop_all();
        self.log(LogKind::Ok, format!("Stopped {n} bot(s)."));
        self.dirty = true;
    }

    /// Flip which side the probe swap takes (Buy ⇄ Sell) and force a redraw so
    /// the status bar and actions pane reflect it. The next `s` dispatches on
    /// the new side.
    fn flip_swap_side(&mut self) {
        self.swap_side = match self.swap_side {
            SwapSide::Buy => SwapSide::Sell,
            SwapSide::Sell => SwapSide::Buy,
        };
        self.log(
            LogKind::Info,
            format!("Swap side set to {}.", swap_side_label(self.swap_side)),
        );
        self.dirty = true;
    }

    /// Enter swap-amount input mode with an empty buffer. Subsequent keys edit
    /// the buffer (see [`App::handle_amount_key`]) until Enter or Esc.
    fn begin_amount_input(&mut self) {
        self.amount_input = Some(String::new());
    }

    /// Edit the swap-amount buffer while in input mode: digits append (capped at
    /// [`MAX_AMOUNT_DIGITS`]), Backspace deletes, Enter commits, Esc cancels.
    fn handle_amount_key(&mut self, k: KeyEvent) {
        let Some(buf) = self.amount_input.as_mut() else {
            return;
        };
        match k.code {
            KeyCode::Char(d @ '0'..='9') => {
                if buf.len() < MAX_AMOUNT_DIGITS {
                    buf.push(d);
                }
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Enter => self.commit_amount_input(),
            KeyCode::Esc => {
                self.amount_input = None;
                self.log(LogKind::Info, "Swap-amount entry cancelled.".to_string());
            }
            _ => {}
        }
    }

    /// Leave input mode, applying the typed buffer to `swap_units` when it
    /// is a positive integer — an empty, zero, or non-numeric buffer is rejected
    /// and the current amount kept.
    fn commit_amount_input(&mut self) {
        let buf = self.amount_input.take().unwrap_or_default();
        match parse_swap_amount(&buf) {
            Some(units) => {
                self.swap_units = units;
                self.log(LogKind::Ok, format!("Swap amount set to {units} units."));
            }
            None => self.log(
                LogKind::Err,
                format!("Invalid swap amount — keeping {} units.", self.swap_units),
            ),
        }
    }

    /// Move the menu selection by `delta`, clamped to the menu bounds.
    fn menu_step(&mut self, delta: isize) {
        let cur = self.menu.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, MENU_LEN as isize - 1) as usize;
        self.menu.select(Some(next));
    }

    /// Run the highlighted menu action.
    fn run_selected(&mut self) {
        let action = action::MENU[self.menu.selected().unwrap_or(0)];
        self.run_action(action);
    }

    /// Step the manual eCLOB spread by ±[`SPREAD_STEP_BPS`] (widen adds, tighten
    /// subtracts), clamp it, then reshape the selected market's ladder to the
    /// new spread. `action` is the widen / tighten variant driving the sign.
    fn step_spread(&mut self, action: Action) {
        self.spread_bps = match action {
            Action::WidenSpread => (self.spread_bps + SPREAD_STEP_BPS).min(MAX_SPREAD_BPS),
            _ => self
                .spread_bps
                .saturating_sub(SPREAD_STEP_BPS)
                .max(MIN_SPREAD_BPS),
        };
        self.run_action(action);
    }

    /// Run `action` — a [`Action::Wipe`] is executed inline (it mutates the
    /// owned validator), everything else is dispatched to a background job.
    /// Shared by the numbered action menu and the eCLOB demo keybinds.
    fn run_action(&mut self, action: Action) {
        let phase = self.chain.phase();
        if !action.enabled(phase) {
            self.log(
                LogKind::Err,
                format!("{} — {}", action.label(), action.disabled_reason(phase)),
            );
            return;
        }
        if action == Action::Wipe {
            self.wipe();
            return;
        }
        if self.job_running {
            self.log(LogKind::Err, "A job is already running.".to_string());
            return;
        }
        self.job_running = true;
        self.log(LogKind::Info, format!("\u{25b6} {}", action.label()));
        action::dispatch(
            action,
            &self.ctx,
            &self.chain,
            self.tx.clone(),
            self.selected_market,
            self.swap_units,
            self.swap_side,
            self.spread_bps,
        );
    }

    /// Kill the validator, wipe its temp ledger, and respawn — then point a
    /// fresh client at it and force a re-poll.
    fn wipe(&mut self) {
        self.log(LogKind::Info, "Wiping localnet…".to_string());
        // The bots quote against the ledger being wiped — stop them so none
        // keeps sending doomed txns at the fresh, empty validator.
        self.bots.stop_all();
        match self.validator.wipe_and_respawn() {
            Ok(()) => {
                self.client = chain::rpc(self.validator.rpc_url());
                // The fresh ledger has no history, so the measured CU costs
                // and the recent fills from the wiped one are stale — clear
                // both panes with it.
                self.cu.clear();
                self.fills.clear();
                self.feed_degraded = false;
                self.dirty = true;
                self.log(
                    LogKind::Ok,
                    "Localnet wiped — validator restarting.".to_string(),
                );
            }
            Err(e) => self.log(LogKind::Err, format!("wipe failed: {e:#}")),
        }
    }

    /// Drain everything the running jobs have queued since the last tick.
    fn drain_jobs(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                JobEvent::Log(s) => {
                    self.note_feed_state(&s);
                    self.log(LogKind::Info, s);
                }
                JobEvent::AccountsChanged => self.dirty = true,
                JobEvent::Cu {
                    label,
                    units,
                    signature,
                } => self.record_cu(label, units, signature),
                JobEvent::Fill { signature, event } => self.record_fill(signature, event),
                JobEvent::Done { ok, summary } => {
                    self.job_running = false;
                    self.dirty = true;
                    self.log(if ok { LogKind::Ok } else { LogKind::Err }, summary);
                }
            }
        }
    }

    /// Re-poll on-chain state if forced (`dirty`) or the interval elapsed.
    /// Also reaps any maker bot that exited on its own, so the per-market
    /// status reflects a crashed or orphaned bot, and clamps the selection to
    /// the current market count (a wipe can drop the markets out from under it).
    fn maybe_refresh(&mut self) {
        if self.dirty || self.last_refresh.elapsed() >= REFRESH_INTERVAL {
            self.chain = accounts::poll(
                &self.client,
                &self.ctx.wallet.pubkey(),
                self.swapper.as_ref(),
                self.selected_market,
            );
            let log = Logger::new(self.tx.clone());
            self.bots.reap(&log);
            self.takers.reap(&log);
            // The taker leaves no on-chain heartbeat (its flow is deliberately
            // quiet between bursts, so activity would flap), but the TUI owns
            // the child — so the swapper reads `Live` exactly when this session
            // is running its taker for the selected market, else `Unknown`.
            // Resolve the symbol before the mutable borrow of `swapper`.
            let taker_live = self
                .selected_symbol()
                .is_some_and(|s| self.takers.is_running(s));
            if taker_live {
                if let Some(swapper) = self.chain.swapper.as_mut() {
                    swapper.liveness = Liveness::Live;
                }
            }
            if !self.chain.markets.is_empty() && self.selected_market >= self.chain.markets.len() {
                self.selected_market = self.chain.markets.len() - 1;
            }
            self.last_refresh = Instant::now();
            self.dirty = false;
        }
    }

    /// Update the FX-feed alert from a streamed maker-bot log line: a
    /// `[feed] coingecko … failed` / `… no prices` line marks the feed down, a
    /// `… recovered` line marks it back up. Keyed on the maker's own deduped
    /// feed messages (`bots/maker-bot`), so the alert flips once per transition,
    /// not per tick.
    fn note_feed_state(&mut self, line: &str) {
        if !line.contains("coingecko") {
            return;
        }
        if line.contains("recovered") {
            self.feed_degraded = false;
        } else if line.contains("failed") || line.contains("no prices") {
            self.feed_degraded = true;
        }
    }

    /// Append a decoded fill to the ring (newest at the back), stamped with the
    /// observed time, trimming to [`FILLS_CAPACITY`], and mark the view dirty so
    /// the fills pane redraws.
    fn record_fill(&mut self, signature: String, event: FillEvent) {
        self.fills.push_back(FillRow {
            time: now_hms(),
            signature,
            event,
        });
        while self.fills.len() > FILLS_CAPACITY {
            self.fills.pop_front();
        }
        self.dirty = true;
    }

    /// Record an operation's measured CU with the signature and time of the
    /// transaction it measured, updating its row in place if the operation has
    /// run before — so the pane shows the latest cost and links to the latest
    /// transaction per label rather than an ever-growing history.
    fn record_cu(&mut self, label: String, units: u64, signature: String) {
        let time = now_hms();
        match self.cu.iter_mut().find(|r| r.label == label) {
            Some(row) => {
                row.units = units;
                row.signature = signature;
                row.time = time;
            }
            None => self.cu.push(CuRow {
                label,
                units,
                signature,
                time,
            }),
        }
    }

    /// Append `text` to the log (and mirror it to the on-disk log),
    /// trimming the in-memory ring to capacity. Multi-line text — e.g. a
    /// program-log stream folded into one error — is split so each line
    /// renders on its own row.
    fn log(&mut self, kind: LogKind, text: String) {
        if let Some(file) = self.log_file.as_mut() {
            let _ = writeln!(file, "{text}");
        }
        for line in text.split('\n') {
            self.log.push_back((kind, line.to_string()));
        }
        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Tear down the explorer container unless Docker was never there, so
        // quitting leaves no orphan — mirrors the owned `Validator`, whose own
        // `Drop` kills its child and wipes its temp ledger right after. `stop`
        // is a no-op if the container isn't up (e.g. quit mid-build).
        if self.ctx.explorer_state.load(Ordering::SeqCst) != explorer::state::NO_DOCKER {
            let _ = explorer::stop(&self.ctx.repo_root);
        }
    }
}

/// The current **local** wall-clock time as `HH:MM:SS` — the timestamp stamped
/// on each fill and CU row as the event loop records it. Uses `libc::localtime_r`
/// (thread-safe, and it applies the system timezone) rather than the `time`
/// crate's local-offset, which refuses to run in a multithreaded process — the
/// TUI has background threads by the time it records anything. Falls back to a
/// zeroed time only if the epoch or conversion fails.
fn now_hms() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    // SAFETY: `localtime_r` writes into our stack `tm` and returns a pointer to
    // it (or null on failure); we read only the plain int fields either way.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Parse a typed swap-amount buffer into whole units — `Some(n)` for a
/// positive integer, `None` for empty, zero, or non-numeric input (the caller
/// keeps the current amount and warns). Split out so it's testable without an
/// `App`.
fn parse_swap_amount(buf: &str) -> Option<u64> {
    match buf.parse::<u64>() {
        Ok(n) if n > 0 => Some(n),
        _ => None,
    }
}

/// Human label for a probe-swap side — for the status bar, the actions pane,
/// and the flip log line.
pub(crate) fn swap_side_label(side: SwapSide) -> &'static str {
    match side {
        SwapSide::Buy => "Buy",
        SwapSide::Sell => "Sell",
    }
}

/// The payload whose click-target rectangle contains `(col, row)`, if any — the
/// lookup behind [`App::address_at`] and [`App::signature_at`], generic over the
/// target payload (an account [`Pubkey`] or a transaction signature `String`)
/// and split out so it's testable without an `App`.
fn hit_target<T: Clone>(targets: &[(Rect, T)], col: u16, row: u16) -> Option<T> {
    let pos = Position { x: col, y: row };
    targets
        .iter()
        .find_map(|(rect, payload)| rect.contains(pos).then(|| payload.clone()))
}

/// RAII guard for the alternate-screen / raw-mode terminal. Restores the
/// terminal on drop (including during a panic unwind).
struct TerminalGuard {
    term: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new(mut term: Terminal<CrosstermBackend<io::Stdout>>) -> Self {
        let _ = enable_raw_mode();
        // Capture the mouse so a click on an account row can open it in the
        // explorer. This does take over the terminal's native click-drag
        // selection; hold Shift (most terminals) to select/copy as usual, and
        // the full log is mirrored to the on-disk file shown in its title
        // regardless.
        let _ = execute!(term.backend_mut(), EnterAlternateScreen, EnableMouseCapture);
        let _ = term.hide_cursor();
        let _ = term.clear();
        Self { term }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.term.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.term.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::{hit_target, parse_swap_amount, swap_side_label, SwapSide};
    use ratatui::layout::Rect;
    use solana_pubkey::Pubkey;

    #[test]
    fn swap_side_label_names_each_side() {
        assert_eq!(swap_side_label(SwapSide::Buy), "Buy");
        assert_eq!(swap_side_label(SwapSide::Sell), "Sell");
    }

    #[test]
    fn parse_swap_amount_accepts_positive_integers_only() {
        assert_eq!(parse_swap_amount("10"), Some(10));
        assert_eq!(parse_swap_amount("1"), Some(1));
        assert_eq!(parse_swap_amount("1000000"), Some(1_000_000));
        // Leading zeros are fine — still a positive integer.
        assert_eq!(parse_swap_amount("007"), Some(7));
        // Empty, zero, and non-numeric input keep the current amount.
        assert_eq!(parse_swap_amount(""), None);
        assert_eq!(parse_swap_amount("0"), None);
        assert_eq!(parse_swap_amount("00"), None);
        assert_eq!(parse_swap_amount("12x"), None);
    }

    #[test]
    fn hit_target_matches_only_the_containing_row() {
        let a = Pubkey::new_from_array([1u8; 32]);
        let b = Pubkey::new_from_array([2u8; 32]);
        // Two stacked single-row targets, side-inset like the accounts pane.
        let targets = [
            (
                Rect {
                    x: 2,
                    y: 3,
                    width: 10,
                    height: 1,
                },
                a,
            ),
            (
                Rect {
                    x: 2,
                    y: 4,
                    width: 10,
                    height: 1,
                },
                b,
            ),
        ];
        // A click inside the first row hits its address; the second, the other.
        assert_eq!(hit_target(&targets, 5, 3), Some(a));
        assert_eq!(hit_target(&targets, 5, 4), Some(b));
        // Right edge is exclusive (x + width), left edge inclusive.
        assert_eq!(hit_target(&targets, 2, 3), Some(a));
        assert_eq!(hit_target(&targets, 12, 3), None);
        // Outside every target — the border column and an empty row.
        assert_eq!(hit_target(&targets, 1, 3), None);
        assert_eq!(hit_target(&targets, 5, 9), None);
        assert_eq!(hit_target::<Pubkey>(&[], 5, 3), None);
    }
}
