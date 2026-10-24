//! Per-market bot child-process supervision.
//!
//! The demo drives the "no liquidity → flash liquidity" reveal from the TUI:
//! each market's bots are child processes the operator starts and stops
//! independently. The maker binary is one supervisor over the whole roster, so
//! a per-market instance is just an invocation scoped with `--market <symbol>`;
//! the taker (opt-in, off by default) is scoped to one book with
//! `--market-address <pda>` (see `bots/maker-bot`, `bots/taker-bot`). This
//! manager owns those children — spawns them, streams their output into the TUI
//! log, notices when one exits, and stops every one on quit (mirroring the
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
use std::time::{Duration, Instant};

/// The bot children of one kind, keyed by their market symbol (the ticker the
/// accounts pane resolves for the market, e.g. `EURC`). A symbol in `children`
/// has a live — or just-exited-and-not-yet-reaped — child; one in `stopping`
/// has been sent SIGTERM and is pulling its liquidity, with the instant past
/// which [`BotManager::reap`] kills it.
#[derive(Default)]
pub struct BotManager {
    children: HashMap<String, Child>,
    stopping: HashMap<String, (Child, Instant)>,
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

    /// Whether the bot for `symbol` has been stopped but not yet exited —
    /// the maker is still taking its book down.
    pub fn is_stopping(&self, symbol: &str) -> bool {
        self.stopping.contains_key(symbol)
    }

    /// How many stopped bots have yet to exit — for the status bar.
    pub fn stopping_count(&self) -> usize {
        self.stopping.len()
    }

    /// Start `cmd` tracked under `symbol`, streaming its output into `log`
    /// tagged with the symbol. A no-op if one is already running, or still
    /// stopping: a fresh maker quoting while the old one zeroes the profile
    /// would have its book pulled out from under it. The command is prebuilt
    /// by [`maker_command`] / [`taker_command`], which set its args and
    /// working directory (`repo_root`, so the checked-in role keys resolve
    /// the same way `make tui` does).
    pub fn start(&mut self, symbol: &str, mut cmd: Command, log: &Logger) -> Result<()> {
        if self.children.contains_key(symbol) {
            log.log(format!("[{symbol}] bot already running"));
            return Ok(());
        }
        if self.stopping.contains_key(symbol) {
            log.log(format!(
                "[{symbol}] bot still stopping — start it once it exits"
            ));
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

    /// Signal the bot for `symbol` to stop and return at once — see
    /// `sigterm` for why a signal. The child moves to the stopping set,
    /// where [`BotManager::reap`] collects it, killing it once `STOP_GRACE`
    /// passes. Returns whether one was running.
    ///
    /// Non-blocking because this runs on the key-handling path: waiting out
    /// the take-off here froze the whole panel — about a second on localnet,
    /// the full grace on a wedged RPC — with nothing on screen to say why.
    pub fn stop(&mut self, symbol: &str) -> bool {
        match self.children.remove(symbol) {
            Some(child) => {
                self.begin_stop(symbol.to_string(), child);
                true
            }
            None => false,
        }
    }

    /// Signal every running bot to stop, as [`BotManager::stop`] does. All
    /// are signalled together, so their take-offs run in parallel.
    pub fn stop_all(&mut self) {
        let children: Vec<_> = self.children.drain().collect();
        for (symbol, child) in children {
            self.begin_stop(symbol, child);
        }
    }

    /// SIGKILL every bot, running or stopping, and reap them — no take-off.
    /// For the wipe, where the ledger a take-off would write to is about to
    /// be destroyed, so waiting for it buys nothing.
    pub fn kill_all(&mut self) {
        let running = self.children.drain().map(|(_, child)| child);
        let stopping = self.stopping.drain().map(|(_, (child, _))| child);
        for mut child in running.chain(stopping) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn begin_stop(&mut self, symbol: String, child: Child) {
        sigterm(&child);
        self.stopping
            .insert(symbol, (child, Instant::now() + STOP_GRACE));
    }

    /// Reap any child that has exited, logging its status: a running one that
    /// exited on its own (so the per-market status reflects a bot that
    /// crashed or lost the validator), and a stopping one that finished its
    /// take-off — or is killed here, its grace spent. Returns whether
    /// anything was reaped (the caller can then redraw).
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
        let now = Instant::now();
        let mut stopped = Vec::new();
        for (symbol, (child, deadline)) in self.stopping.iter_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    log.log(format!("[{symbol}] bot stopped ({status})"));
                    stopped.push(symbol.clone());
                }
                Ok(None) if now < *deadline => {}
                // An unreadable status, or the grace spent: kill and reap, so
                // no child outlives its manager.
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    log.log(format!(
                        "[{symbol}] bot still up {}s after SIGTERM — killed",
                        STOP_GRACE.as_secs()
                    ));
                    stopped.push(symbol.clone());
                }
            }
        }
        for symbol in &stopped {
            self.stopping.remove(symbol);
        }
        !exited.is_empty() || !stopped.is_empty()
    }
}

/// How long a stopped bot gets to exit on its own after SIGTERM before it is
/// killed. The maker spends it pulling its liquidity — a confirmed kill stamp
/// and a confirmed zeroed profile per market — so this is sized for two
/// confirmations, with room for a slow RPC.
const STOP_GRACE: Duration = Duration::from_secs(20);

/// Ask `child` to stop.
///
/// SIGTERM rather than `Child::kill` (which is SIGKILL) because the maker
/// treats SIGTERM as "pull the liquidity, then exit" — stopping a market's
/// bot from the demo *is* taking its book down. A SIGKILL skips that and
/// leaves the book resting until its levels expire on their own TIF. The
/// taker has no handler, so SIGTERM ends it at once.
fn sigterm(child: &Child) {
    // SAFETY: `kill` takes a pid and a signal number and touches no memory
    // of ours. The pid is a child we have not yet reaped, so it cannot have
    // been recycled for an unrelated process.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
}

/// Wait out `children`, each already sent SIGTERM, until its deadline, then
/// SIGKILL whatever is left, and reap every one.
fn wait_out(children: Vec<(Child, Instant)>) {
    for (mut child, deadline) in children {
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                // An unreadable status, or the grace spent: kill and reap, so
                // no child outlives its manager.
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}

impl Drop for BotManager {
    fn drop(&mut self) {
        // Leave no orphaned bots when the TUI quits or panics. This one path
        // still blocks: the process is exiting, so there is no later reap to
        // hand the take-offs to, and a quit mid-take-off must not strand a
        // book.
        self.stop_all();
        wait_out(self.stopping.drain().map(|(_, entry)| entry).collect());
    }
}

/// The localnet market-data store, as reached **from the host**.
///
/// The compose stack publishes Postgres on `127.0.0.1:5432` and carries the
/// same throwaway localnet credentials in
/// `infra/localnet/docker-compose.yml`; a containerized bot reaches it as
/// `postgres:5432` on the compose network, and a bot this panel spawns is a
/// host process, so it needs the published address instead.
const LOCALNET_STORE_URL: &str = "postgres://dropset:dropset@127.0.0.1:5432/dropset";

/// The variable the maker reads for the store. Spelled here rather than
/// imported from the maker crate: the TUI does not depend on it, and adding
/// that dependency to share one string would pull the whole bot into this
/// crate's build graph.
const STORE_URL_ENV: &str = "DROPSET_DATABASE_URL";

/// Build the command that runs the maker bot scoped to one `symbol` (the maker
/// resolves the symbol to its market itself). See `bot_command` for how the
/// binary is located.
///
/// **Passes the store URL through, defaulting to the localnet one.** The maker
/// reads its intraday FX anchor from the market-data store and refuses to
/// start without a URL, which is deliberate — that anchor is on the price path
/// and a maker quoting without it would be pricing off a daily fix. But a
/// child process only inherits what this panel's own shell happens to carry,
/// and nothing sets this variable interactively, so every maker launched from
/// here failed at startup until this defaulted it. The Docker path never hit
/// it because compose supplies the variable.
///
/// An ambient value still wins, so pointing the demo at another store stays a
/// matter of exporting it before launching the TUI — the same precedence
/// compose uses.
pub fn maker_command(repo_root: &Path, symbol: &str, rpc_url: &str) -> Command {
    let mut cmd = bot_command(
        repo_root,
        "dropset-maker-bot",
        &["--market", symbol, "--rpc", rpc_url],
    );
    if std::env::var_os(STORE_URL_ENV).is_none() {
        cmd.env(STORE_URL_ENV, LOCALNET_STORE_URL);
    }
    cmd
}

/// Build the command that runs the taker bot scoped to one market by its PDA
/// `address` (the taker has no symbol roster, so the TUI passes the selected
/// market's address directly). See `bot_command` for how the binary is
/// located.
pub fn taker_command(repo_root: &Path, address: &Pubkey, rpc_url: &str) -> Command {
    bot_command(
        repo_root,
        "dropset-taker-bot",
        &["--market-address", &address.to_string(), "--rpc", rpc_url],
    )
}

/// Build a command running the bot binary `bin_name` with `args` from the repo
/// root (working directory `repo_root`, so the role-key paths resolve).
///
/// Prefers the prebuilt binary in `target/debug/`: `make tui` / `tui-prebuild`
/// builds the bots up front, so the fast path is a direct fork+exec. This
/// matters most when starting every market's bot at once — routing each launch
/// through `cargo run` spawns N cargo processes that contend on the build lock
/// and serialize, so "start all makers" stalls on a row of "starting…" while
/// each waits its turn for the lock. Exec-ing the built binary sidesteps that.
///
/// The stale-binary hazard the cargo path guarded against — an old binary
/// (built against a superseded `MarketHeader` size) decoding a current market
/// at the wrong offsets and dying with `SectorOverflow` — is closed by the
/// prebuild: the same `make tui` that launches the panel rebuilds the bots
/// first, so the binary on disk matches the deployed program. The cargo
/// fallback only fires when the binary is **absent** (a direct
/// `cargo run -p dropset-tui`, skipping the prebuild) — it rebuilds and runs a
/// current binary. Under the shared cargo pool the prebuild copies the bots
/// here from the pool's `debug/`, which every worktree overwrites, so this
/// path stays this worktree's own build. The
/// one gap it does *not* cover: a binary left over from an earlier `make tui`
/// is exec'd as-is, so editing bot/program sources and then launching via a
/// bare `cargo run -p dropset-tui` (no fresh prebuild) can run it stale. The
/// standard `make tui` / `make demo` flow rebuilds first, so this only bites a
/// deliberately-skipped prebuild.
fn bot_command(repo_root: &Path, bin_name: &str, args: &[&str]) -> Command {
    let built = repo_root.join("target").join("debug").join(bin_name);
    let mut cmd = if built.exists() {
        Command::new(built)
    } else {
        let mut c = Command::new("cargo");
        c.args(["run", "--quiet", "-p", bin_name, "--"]);
        c
    };
    cmd.args(args).current_dir(repo_root);
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

#[cfg(test)]
mod tests {
    use super::{BotManager, LOCALNET_STORE_URL, STOP_GRACE, STORE_URL_ENV};
    use crate::job::{JobEvent, Logger};
    use std::process::Command;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn sleeping(symbols: &[&str], log: &Logger) -> BotManager {
        let mut bots = BotManager::new();
        for symbol in symbols {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            bots.start(symbol, cmd, log).expect("spawn sleep");
        }
        bots
    }

    /// A stop signals and returns; the reap collects the exit. A child that
    /// honours SIGTERM is reaped well inside the grace period, not after it.
    #[test]
    fn a_stop_returns_at_once_and_the_reap_collects_it() {
        let (tx, rx) = mpsc::channel();
        let log = Logger::new(tx);
        let mut bots = sleeping(&["A", "B"], &log);
        let started = Instant::now();
        assert!(bots.stop("A"));
        bots.stop_all();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(bots.running_count(), 0);
        assert_eq!(bots.stopping_count(), 2);
        assert!(bots.is_stopping("A") && !bots.is_running("A"));
        while bots.stopping_count() > 0 {
            assert!(
                started.elapsed() < STOP_GRACE / 4,
                "{:?}",
                started.elapsed()
            );
            bots.reap(&log);
            thread::sleep(Duration::from_millis(20));
        }
        let stopped = rx
            .try_iter()
            .filter(|ev| matches!(ev, JobEvent::Log(s) if s.contains("bot stopped")))
            .count();
        assert_eq!(stopped, 2);
    }

    /// A symbol still taking its book down is not restarted under it.
    #[test]
    fn a_stopping_bot_is_not_restarted() {
        let (tx, _rx) = mpsc::channel();
        let log = Logger::new(tx);
        let mut bots = sleeping(&["A"], &log);
        bots.stop("A");
        bots.start("A", Command::new("true"), &log).expect("no-op");
        assert!(!bots.is_running("A"));
        bots.kill_all();
        assert_eq!(bots.stopping_count(), 0);
    }

    /// The compose file is the third copy of both strings, and the one that
    /// actually has to agree with them.
    ///
    /// Neither constant can be imported: the variable's canonical definition
    /// lives in the maker crate, which this one deliberately does not depend
    /// on, and the URL is compose's. So nothing the compiler checks connects
    /// the three — rename the canonical const and every TUI-spawned maker
    /// fails at startup with no build error, which is precisely the bug this
    /// module was changed to fix.
    ///
    /// Pinning against the committed artifact costs one `include_str!` and
    /// no dependency. It is a weaker guarantee than a shared symbol and a
    /// strictly better one than nothing.
    #[test]
    fn the_store_url_and_its_variable_match_the_compose_file() {
        let compose = include_str!("../../infra/localnet/docker-compose.yml");
        assert!(
            compose.contains(STORE_URL_ENV),
            "{STORE_URL_ENV} is not named in the compose file — the canonical \
             constant in the maker crate has probably been renamed"
        );
        // The host form differs from compose's in-network form only by host
        // and port, so pin the parts that must agree: the credentials and the
        // database name.
        for part in ["dropset:dropset", "/dropset"] {
            assert!(
                LOCALNET_STORE_URL.contains(part),
                "the localnet store URL no longer carries {part:?}"
            );
            assert!(
                compose.contains(part),
                "the compose file no longer carries {part:?} — the TUI would \
                 hand its makers a URL the stack does not accept"
            );
        }
    }
}
