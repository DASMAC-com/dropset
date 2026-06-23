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
use crate::job::{self, Logger};
use anyhow::{Context, Result};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use std::path::{Path, PathBuf};
use std::process::Command;

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

    // Materialize the program keypair from its committed source so anchor
    // build's program-id check sees keypair == declare_id! (keys/AAAA.json
    // is canonical; target/deploy/ is a pure build artifact).
    let program_keypair: PathBuf = repo_root.join("target/deploy/dropset-keypair.json");
    std::fs::create_dir_all(repo_root.join("target/deploy")).context("create target/deploy")?;
    std::fs::copy(repo_root.join("keys/AAAA.json"), &program_keypair)
        .context("stage program keypair")?;

    let mut sync = Command::new("anchor");
    sync.args(["keys", "sync"]).current_dir(repo_root);
    job::run_streaming(log, "anchor keys sync", sync)?;

    // `--no-idl` skips IDL generation, which builds and runs the test suite
    // to extract the IDL. Deploy only needs the `.so`, and the IDL is
    // already checked in (sdk/idl/dropset.json).
    let mut build = Command::new("anchor");
    build.args(["build", "--no-idl"]).current_dir(repo_root);
    job::run_streaming(log, "anchor build --no-idl", build)?;

    let so: PathBuf = repo_root.join("target/deploy/dropset.so");
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
    job::run_streaming(log, "solana program deploy", deploy)?;

    log.accounts_changed();
    Ok("Program deployed".into())
}

/// Close the deployed program, reclaiming its (program-data) rent to
/// `recipient`. Signed by the wallet at `wallet_path` (the upgrade
/// authority). The program account is closed, so the phase drops back to
/// `ProgramAbsent` — a fresh deploy needs a wiped validator.
pub fn close_program(
    log: &Logger,
    rpc_url: &str,
    wallet_path: &str,
    recipient: &Pubkey,
) -> Result<()> {
    let mut close = Command::new("solana");
    close
        .args(["program", "close"])
        .arg(dropset_sdk::DROPSET_ID.to_string())
        .arg("--recipient")
        .arg(recipient.to_string())
        .arg("--keypair")
        .arg(wallet_path)
        .arg("--url")
        .arg(rpc_url)
        .arg("--bypass-warning");
    job::run_streaming(log, "solana program close", close)
}
