//! Application state and the synchronous event loop.
//!
//! The loop mirrors the `anchor debugger`'s: draw, then service input with a
//! burst-drain so a held key collapses into one render. On top of that it
//! also drains background-job events and re-polls on-chain state on a timer
//! (or immediately when a job signals a change), so the panel tracks the
//! validator without a blocking read. A [`TerminalGuard`] restores the
//! terminal on the way out; the owned [`Validator`] kills the child and
//! wipes its temp ledger on `Drop`, so quitting leaves no orphan.

use crate::accounts::{self, ChainState};
use crate::action::{self, Action, JobContext};
use crate::chain;
use crate::job::JobEvent;
use crate::ui;
use crate::validator::Validator;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, widgets::ListState, Terminal};
use solana_client::rpc_client::RpcClient;
use solana_signer::Signer;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
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
    /// Path the log is mirrored to on disk (shown in the log pane title).
    pub(crate) log_path: PathBuf,
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
        Ok(Self {
            ctx,
            validator,
            client,
            chain: ChainState::default(),
            menu,
            log: VecDeque::new(),
            job_running: false,
            log_path,
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

    fn handle_event(&mut self, ev: Event) -> Flow {
        let Event::Key(k) = ev else {
            return Flow::Continue;
        };
        self.handle_key(k)
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
            self.chain = accounts::poll(&self.client, &self.ctx.wallet.pubkey());
            self.last_refresh = Instant::now();
            self.dirty = false;
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

/// RAII guard for the alternate-screen / raw-mode terminal. Restores the
/// terminal on drop (including during a panic unwind).
struct TerminalGuard {
    term: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new(mut term: Terminal<CrosstermBackend<io::Stdout>>) -> Self {
        let _ = enable_raw_mode();
        // No mouse capture: the panel takes no mouse input, and capturing it
        // would steal the terminal's native text selection — so leaving it
        // off keeps the log copy-pasteable.
        let _ = execute!(term.backend_mut(), EnterAlternateScreen);
        let _ = term.hide_cursor();
        let _ = term.clear();
        Self { term }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.term.backend_mut(), LeaveAlternateScreen);
        let _ = self.term.show_cursor();
    }
}
