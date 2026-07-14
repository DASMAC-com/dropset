//! Per-market bot child-process supervision.
//!
//! The demo drives the "no liquidity → flash liquidity" reveal from the TUI:
//! each market's bots are child processes the operator starts and stops
//! independently. The maker binary is one supervisor over the whole roster, so
//! a per-market instance is just an invocation scoped with `--market <symbol>`;
//! the taker (opt-in, off by default) is scoped to one book with
//! `--market-address <pda>` (see `bots/maker-bot`, `bots/taker-bot`). This
//! manager owns those children — spawns them, streams their output into the TUI
//! log, notices when one exits, and kills every one on quit (mirroring the
//! owned [`crate::validator::Validator`] and the managed explorer container).
//! The `App` holds one manager per bot kind (maker, taker), each keyed by the
//! market symbol, so a maker and a taker for the same market coexist.
//!
//! Output goes to the log pane, never the inherited stdio — a child writing to
//! the real stdout would corrupt the alternate-screen TUI — so each line is
//! piped, tagged with its market symbol, and forwarded over the job channel.

use crate::job::Logger;
use anyhow::{Context, Result};
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;

/// The running bot children of one kind, keyed by their market symbol (the
/// ticker the accounts pane resolves for the market, e.g. `EURC`). A symbol
/// present here has a live — or just-exited-and-not-yet-reaped — child.
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

    /// Start `cmd` tracked under `symbol`, streaming its output into `log`
    /// tagged with the symbol. A no-op if one is already running. The command
    /// is prebuilt by [`maker_command`] / [`taker_command`], which set its args
    /// and working directory (`repo_root`, so the checked-in role keys resolve
    /// the same way `make tui` does).
    pub fn start(&mut self, symbol: &str, mut cmd: Command, log: &Logger) -> Result<()> {
        if self.children.contains_key(symbol) {
            log.log(format!("[{symbol}] bot already running"));
            return Ok(());
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn bot for {symbol}"))?;
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
        // Leave no orphaned bots when the TUI quits or panics.
        self.stop_all();
    }
}

/// Build the command that runs the maker bot scoped to one `symbol` (the maker
/// resolves the symbol to its market itself). See [`bot_command`] for how the
/// binary is located.
pub fn maker_command(repo_root: &Path, symbol: &str, rpc_url: &str) -> Command {
    bot_command(
        repo_root,
        "dropset-maker-bot",
        &["--market", symbol, "--rpc", rpc_url],
    )
}

/// Build the command that runs the taker bot scoped to one market by its PDA
/// `address` (the taker has no symbol roster, so the TUI passes the selected
/// market's address directly). See [`bot_command`] for how the binary is
/// located.
pub fn taker_command(repo_root: &Path, address: &Pubkey, rpc_url: &str) -> Command {
    bot_command(
        repo_root,
        "dropset-taker-bot",
        &["--market-address", &address.to_string(), "--rpc", rpc_url],
    )
}

/// Build a command running the bot binary `bin_name` with `args`, always
/// through `cargo run` from the repo root (working directory `repo_root`, so
/// the role-key paths resolve). cargo rebuilds the bot incrementally if its
/// sources moved and otherwise runs the built binary unchanged — so the spawned
/// bot is always current with the deployed program's account layout. `make tui`
/// builds the bots up front, so in practice this is a no-op that just launches
/// the fresh binary.
///
/// Launching a prebuilt binary sitting in the target dir directly would be a
/// hazard: a stale one (built against a superseded `MarketHeader` size) decodes
/// a current market at the wrong offsets and dies with `SectorOverflow`. Going
/// through cargo makes that impossible — a drifted binary is rebuilt, not run.
fn bot_command(repo_root: &Path, bin_name: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--quiet", "-p", bin_name, "--"])
        .args(args)
        .current_dir(repo_root);
    cmd
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
