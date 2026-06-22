//! The deploy action: build the program and publish it to the localnet.
//!
//! Build goes through the repo's `make program` target rather than raw
//! `anchor` calls, so it inherits the toolchain pre-flight
//! (`check-toolchain`) and the program-keypair materialization
//! (`keys/AAAA.json` → `target/deploy/dropset-keypair.json`) — the latter
//! is what pins the deployed program id to [`dropset_sdk::DROPSET_ID`].
//! Then `solana program deploy` publishes the `.so` with the wallet as both
//! payer and upgrade authority — the `init` handler requires payer ==
//! upgrade authority, so deploy and init must use the same key.

use crate::chain;
use crate::job::Logger;
use anyhow::{bail, Context, Result};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

/// Deploy funds airdropped to the wallet before publishing — comfortably
/// covers program-account rent plus fees on localnet.
const DEPLOY_AIRDROP_SOL: u64 = 1_000;

/// Build the program and deploy it to `rpc_url`, signed by the wallet at
/// `wallet_path` (also the upgrade authority). Streams build + deploy
/// output into the log.
pub fn deploy_program(
    log: &Logger,
    repo_root: &Path,
    rpc_url: &str,
    wallet_path: &str,
    wallet_pubkey: &Pubkey,
) -> Result<String> {
    let client = chain::rpc(rpc_url);
    log.log("Airdropping deploy funds to the wallet…");
    chain::airdrop(
        &client,
        wallet_pubkey,
        DEPLOY_AIRDROP_SOL * LAMPORTS_PER_SOL,
    )
    .context("fund wallet for deploy")?;

    let mut build = Command::new("make");
    build.arg("program").current_dir(repo_root);
    run_streaming(log, "make program", build)?;

    let so: PathBuf = repo_root.join("target/deploy/dropset.so");
    let program_keypair: PathBuf = repo_root.join("target/deploy/dropset-keypair.json");
    let mut deploy = Command::new("solana");
    deploy
        .args(["program", "deploy"])
        .arg(&so)
        .arg("--program-id")
        .arg(&program_keypair)
        .arg("--keypair")
        .arg(wallet_path)
        .arg("--url")
        .arg(rpc_url)
        .arg("--upgrade-authority")
        .arg(wallet_path)
        .current_dir(repo_root);
    run_streaming(log, "solana program deploy", deploy)?;

    log.accounts_changed();
    Ok("Program deployed".into())
}

/// Run `cmd`, streaming both stdout and stderr into the log line-by-line.
/// Returns `Err` if the process exits non-zero. `label` is the banner shown
/// before the output and named in a failure.
fn run_streaming(log: &Logger, label: &str, mut cmd: Command) -> Result<()> {
    log.log(format!("$ {label}"));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn `{label}` (is the toolchain installed?)"))?;

    // stderr on its own reader thread, stdout on this one — both feed the
    // same (cloneable) Logger, interleaving as the process emits them.
    let stderr = child.stderr.take().expect("piped stderr");
    let err_log = log.clone();
    let err_thread = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            err_log.log(line);
        }
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            log.log(line);
        }
    }
    let _ = err_thread.join();

    let status = child
        .wait()
        .with_context(|| format!("wait for `{label}`"))?;
    if !status.success() {
        bail!("`{label}` exited with {status}");
    }
    Ok(())
}
