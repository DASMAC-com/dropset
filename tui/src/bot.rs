//! Per-market `dropset-maker-bot` child-process supervision.
//!
//! The demo drives the "no liquidity → flash liquidity" reveal from the TUI:
//! each market's maker bot is a child process the operator starts and stops
//! independently. The bot binary is one supervisor over the whole roster, so a
//! per-market instance is just an invocation scoped with `--market <symbol>`
//! (see `bots/maker-bot`); this manager owns those children — spawns them,
//! streams their output into the TUI log, notices when one exits, and kills
//! every one on quit (mirroring the owned [`crate::validator::Validator`] and
//! the managed explorer container).
//!
//! Output goes to the log pane, never the inherited stdio — a child writing to
//! the real stdout would corrupt the alternate-screen TUI — so each line is
//! piped, tagged with its market symbol, and forwarded over the job channel.

use crate::job::Logger;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;

/// The running maker-bot children, keyed by their market symbol (the ticker
/// the accounts pane resolves for the market, e.g. `EURC`). A symbol present
/// here has a live — or just-exited-and-not-yet-reaped — child.
#[derive(Default)]
pub struct BotManager {
    children: HashMap<String, Child>,
}

impl BotManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a bot for `symbol` is currently tracked as running. Reflects
    /// liveness only as of the last [`BotManager::reap`], which prunes children
    /// that have exited on their own (a crash, or the localnet going away).
    pub fn is_running(&self, symbol: &str) -> bool {
        self.children.contains_key(symbol)
    }

    /// How many bots are currently running — for the status bar.
    pub fn running_count(&self) -> usize {
        self.children.len()
    }

    /// Start the maker bot for `symbol` against `rpc_url`, streaming its output
    /// into `log` tagged with the symbol. A no-op if one is already running.
    /// The child runs from `repo_root` so it resolves the checked-in leader key
    /// (`keys/EEEE.json`) the same way `make tui` does.
    pub fn start(
        &mut self,
        symbol: &str,
        repo_root: &Path,
        rpc_url: &str,
        log: &Logger,
    ) -> Result<()> {
        if self.children.contains_key(symbol) {
            log.log(format!("[{symbol}] bot already running"));
            return Ok(());
        }
        let mut cmd = maker_bot_command(repo_root, symbol, rpc_url);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn maker bot for {symbol}"))?;
        stream(child.stdout.take(), symbol, log.clone());
        stream(child.stderr.take(), symbol, log.clone());
        self.children.insert(symbol.to_string(), child);
        log.log(format!("[{symbol}] bot starting…"));
        Ok(())
    }

    /// Stop the bot for `symbol`, killing and reaping the child. Returns
    /// whether one was running.
    pub fn stop(&mut self, symbol: &str) -> bool {
        match self.children.remove(symbol) {
            Some(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                true
            }
            None => false,
        }
    }

    /// Stop every running bot. Used by "stop all" and, indirectly, by `Drop`.
    pub fn stop_all(&mut self) {
        for (_, mut child) in self.children.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Reap any child that has exited on its own, logging its status, so the
    /// per-market status reflects a bot that crashed or lost the validator.
    /// Returns whether anything was reaped (the caller can then redraw).
    pub fn reap(&mut self, log: &Logger) -> bool {
        let mut exited = Vec::new();
        for (symbol, child) in self.children.iter_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                log.log(format!("[{symbol}] bot exited ({status})"));
                exited.push(symbol.clone());
            }
        }
        for symbol in &exited {
            self.children.remove(symbol);
        }
        !exited.is_empty()
    }
}

impl Drop for BotManager {
    fn drop(&mut self) {
        // Leave no orphaned maker bots when the TUI quits or panics.
        self.stop_all();
    }
}

/// Build the command that runs the maker bot scoped to one `symbol`. Prefers a
/// `dropset-maker-bot` binary sitting beside the running TUI (the same cargo
/// target dir, when it has been built), and otherwise falls back to `cargo run`
/// from the repo root — which builds it on first use, streaming the build into
/// the log. Either way the working directory is `repo_root` so the leader key
/// path resolves.
fn maker_bot_command(repo_root: &Path, symbol: &str, rpc_url: &str) -> Command {
    match sibling_binary() {
        Some(bin) => {
            let mut cmd = Command::new(bin);
            cmd.args(["--market", symbol, "--rpc", rpc_url])
                .current_dir(repo_root);
            cmd
        }
        None => {
            let mut cmd = Command::new("cargo");
            cmd.args([
                "run",
                "--quiet",
                "-p",
                "dropset-maker-bot",
                "--",
                "--market",
                symbol,
                "--rpc",
                rpc_url,
            ])
            .current_dir(repo_root);
            cmd
        }
    }
}

/// A `dropset-maker-bot` executable next to the current one (same target dir),
/// if it has been built — otherwise `None`, steering the caller to `cargo run`.
fn sibling_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin = exe.with_file_name("dropset-maker-bot");
    bin.exists().then_some(bin)
}

/// Forward a child pipe's lines into `log`, each tagged `[symbol]`, on a
/// detached thread so the event loop never blocks on the bot's output.
fn stream<R: Read + Send + 'static>(pipe: Option<R>, symbol: &str, log: Logger) {
    let Some(pipe) = pipe else {
        return;
    };
    let symbol = symbol.to_string();
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            log.log(format!("[{symbol}] {line}"));
        }
    });
}
