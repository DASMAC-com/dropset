//! `dropset-demo-check` — the pre-demo smoke check.
//!
//! Runs [`demo_check::run`] against a `make demo` stack that is already up
//! and prints one line per surface: pass, fail, or a named expected state
//! (keyless mode, the FX weekend), each with its evidence. Exits non-zero on
//! any fail. Reads only — no transaction, no database write, no container
//! touched.
//!
//! ```text
//! dropset-demo-check [--rpc-url <url>] [--database-url <url>]
//! ```
//!
//! - `--rpc-url <url>` — the localnet validator (default
//!   `http://127.0.0.1:8899`).
//! - `--database-url <url>` — the shared store, as the read-only role
//!   (default the stack's `dropset_ro` on `127.0.0.1:5432`).

use anyhow::{anyhow, bail, Result};
use dropset_tui::demo_check::{self, Targets, Verdict, DEFAULT_DATABASE_URL};
use dropset_tui::validator;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> Result<ExitCode> {
    let mut rpc_url = validator::DEFAULT_RPC_URL.to_string();
    let mut database_url = DEFAULT_DATABASE_URL.to_string();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rpc-url" => {
                rpc_url = it
                    .next()
                    .ok_or_else(|| anyhow!("--rpc-url needs a value"))?
            }
            "--database-url" => {
                database_url = it
                    .next()
                    .ok_or_else(|| anyhow!("--database-url needs a value"))?
            }
            "--help" | "-h" => {
                println!("dropset-demo-check [--rpc-url <url>] [--database-url <url>]");
                return Ok(ExitCode::SUCCESS);
            }
            other => bail!("unknown argument: {other} (try --help)"),
        }
    }
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("locate repo root from crate dir"))?
        .to_path_buf();
    let checks = demo_check::run(&Targets {
        repo_root: &repo_root,
        rpc_url: &rpc_url,
        database_url: &database_url,
    });
    for check in &checks {
        println!("{check}");
    }
    let failed = checks.iter().filter(|c| c.verdict == Verdict::Fail).count();
    if failed == 0 {
        println!("\ndemo stack ready");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\n{failed} surface(s) failed — see docs/demo.md for what each line means");
        Ok(ExitCode::FAILURE)
    }
}
