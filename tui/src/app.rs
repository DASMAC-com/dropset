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

use crate::accounts::{self, ChainState};
use crate::action::{self, Action, JobContext};
use crate::chain;
use crate::explorer;
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
use ratatui::{backend::CrosstermBackend, layout::Rect, widgets::ListState, Terminal};
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

/// Kind of a log line — drives its color.
#[derive(Clone, Copy)]
pub enum LogKind {
    Info,
    Ok,
    Err,
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
    /// Measured compute-unit cost per operation, in first-seen order — one
    /// row per `label`, updated in place when an operation runs again. Drives
    /// the CU pane.
    pub(crate) cu: Vec<(String, u64)>,
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
    /// Address click targets for the accounts pane, rebuilt by [`ui::draw`]
    /// each frame: a left-click inside one of these rectangles opens that
    /// account in the explorer.
    pub(crate) click_targets: Vec<(Rect, Pubkey)>,
    tx: Sender<JobEvent>,
    rx: Receiver<JobEvent>,
    /// Append handle for the on-disk log mirror, if it opened.
    log_file: Option<File>,
    last_refresh: Instant,
    dirty: bool,
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
            click_targets: Vec::new(),
            tx,
            rx,
            log_file,
            // Force an immediate first poll.
            last_refresh: Instant::now() - REFRESH_INTERVAL,
            dirty: true,
        })
    }

    /// Run the event loop until the user quits.
    pub fn run(&mut self) -> Result<()> {
        self.log(LogKind::Info, "Starting solana-test-validator…".to_string());
        self.start_explorer();
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

    /// Handle a mouse event: a left-click on an account row (a [`ui`] click
    /// target) opens that account in the explorer; everything else is ignored.
    fn handle_mouse(&mut self, m: MouseEvent) {
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }
        if let Some(addr) = self.address_at(m.column, m.row) {
            self.open_in_explorer(addr);
        }
    }

    /// The account address whose click-target rect contains `(col, row)`, if
    /// any.
    fn address_at(&self, col: u16, row: u16) -> Option<Pubkey> {
        self.click_targets.iter().find_map(|(rect, addr)| {
            let hit = col >= rect.x
                && col < rect.x + rect.width
                && row >= rect.y
                && row < rect.y + rect.height;
            hit.then_some(*addr)
        })
    }

    /// Open `address` in the explorer — the local container when it's ready,
    /// else the hosted explorer as a fallback (which can't reach the localnet
    /// in some browsers; see [`explorer`]). Best-effort: a launch failure is
    /// logged, not fatal.
    fn open_in_explorer(&mut self, address: Pubkey) {
        let url = if self.ctx.explorer_state.load(Ordering::SeqCst) == explorer::state::READY {
            explorer::account_url(&address, &self.ctx.rpc_url)
        } else {
            explorer::hosted_account_url(&address, &self.ctx.rpc_url)
        };
        self.log(LogKind::Info, format!("Opening {address} in the explorer…"));
        if let Err(e) = open::that(&url) {
            self.log(LogKind::Err, format!("open explorer: {e:#}"));
        }
    }

    fn handle_key(&mut self, k: KeyEvent) -> Flow {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Flow::Quit,
            KeyCode::Char('c') if ctrl => return Flow::Quit,
            KeyCode::Char('j') | KeyCode::Down => self.menu_step(1),
            KeyCode::Char('k') | KeyCode::Up => self.menu_step(-1),
            KeyCode::Char('r') => self.dirty = true,
            KeyCode::Enter => self.run_selected(),
            KeyCode::Char(d @ '1'..='9') => {
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

    /// Move the menu selection by `delta`, clamped to the menu bounds.
    fn menu_step(&mut self, delta: isize) {
        let cur = self.menu.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, MENU_LEN as isize - 1) as usize;
        self.menu.select(Some(next));
    }

    /// Run the selected action — but a [`Action::Wipe`] is executed inline
    /// (it mutates the owned validator), everything else is dispatched to a
    /// background job.
    fn run_selected(&mut self) {
        let action = action::MENU[self.menu.selected().unwrap_or(0)];
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
        action::dispatch(action, &self.ctx, &self.chain, self.tx.clone());
    }

    /// Kill the validator, wipe its temp ledger, and respawn — then point a
    /// fresh client at it and force a re-poll.
    fn wipe(&mut self) {
        self.log(LogKind::Info, "Wiping localnet…".to_string());
        match self.validator.wipe_and_respawn() {
            Ok(()) => {
                self.client = chain::rpc(self.validator.rpc_url());
                // The fresh ledger has no history, so the measured CU costs
                // from the wiped one are stale — clear the pane with it.
                self.cu.clear();
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
                JobEvent::Log(s) => self.log(LogKind::Info, s),
                JobEvent::AccountsChanged => self.dirty = true,
                JobEvent::Cu { label, units } => self.record_cu(label, units),
                JobEvent::Done { ok, summary } => {
                    self.job_running = false;
                    self.dirty = true;
                    self.log(if ok { LogKind::Ok } else { LogKind::Err }, summary);
                }
            }
        }
    }

    /// Re-poll on-chain state if forced (`dirty`) or the interval elapsed.
    fn maybe_refresh(&mut self) {
        if self.dirty || self.last_refresh.elapsed() >= REFRESH_INTERVAL {
            self.chain = accounts::poll(
                &self.client,
                &self.ctx.wallet.pubkey(),
                self.swapper.as_ref(),
            );
            self.last_refresh = Instant::now();
            self.dirty = false;
        }
    }

    /// Record an operation's measured CU, updating its row in place if the
    /// operation has run before so the pane shows the latest cost per label
    /// rather than an ever-growing history.
    fn record_cu(&mut self, label: String, units: u64) {
        match self.cu.iter_mut().find(|(l, _)| *l == label) {
            Some(entry) => entry.1 = units,
            None => self.cu.push((label, units)),
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
