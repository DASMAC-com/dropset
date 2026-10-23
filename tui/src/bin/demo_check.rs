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

/// `Result<ExitCode>` rather than `Result<()>`: a failed surface is a
/// non-zero exit without being an error.
fn main() -> Result<ExitCode> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        print_help();
        return Ok(ExitCode::SUCCESS);
    }
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("locate repo root from crate dir"))?
        .to_path_buf();
    let checks = demo_check::run(&Targets {
        repo_root: &repo_root,
        rpc_url: args
            .rpc_url
            .as_deref()
            .unwrap_or(validator::DEFAULT_RPC_URL),
        database_url: args.database_url.as_deref().unwrap_or(DEFAULT_DATABASE_URL),
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

fn print_help() {
    println!(
        "dropset-demo-check — the pre-demo smoke check\n\n\
         USAGE:\n    \
         dropset-demo-check [--rpc-url <url>] [--database-url <url>]\n\n\
         OPTIONS:\n        \
         --rpc-url <url>        localnet validator (default: http://127.0.0.1:8899)\n        \
         --database-url <url>   the store, read-only (default: the stack's dropset_ro)\n    \
         -h, --help                 show this help"
    );
}

/// Consume the value following an option, rejecting a missing value or a
/// flag-looking token (so `--rpc-url --database-url x` errors instead of
/// silently taking `--database-url` as the URL).
fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    match it.next() {
        Some(v) if !v.starts_with('-') => Ok(v),
        Some(v) => bail!("{flag} needs a value, got flag-like `{v}`"),
        None => bail!("{flag} needs a value"),
    }
}

/// Parsed command line.
struct Args {
    rpc_url: Option<String>,
    database_url: Option<String>,
    help: bool,
}

impl Args {
    fn parse(mut it: impl Iterator<Item = String>) -> Result<Self> {
        let mut a = Args {
            rpc_url: None,
            database_url: None,
            help: false,
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--rpc-url" => a.rpc_url = Some(value(&mut it, "--rpc-url")?),
                "--database-url" => a.database_url = Some(value(&mut it, "--database-url")?),
                "--help" | "-h" => a.help = true,
                other => bail!("unknown argument: {other} (try --help)"),
            }
        }
        Ok(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args> {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn args_parse_flags_and_values() {
        let a = parse(&["--rpc-url", "http://h:1", "--database-url", "postgres://x"]).unwrap();
        assert_eq!(a.rpc_url.as_deref(), Some("http://h:1"));
        assert_eq!(a.database_url.as_deref(), Some("postgres://x"));
        assert!(!a.help);
    }

    #[test]
    fn args_parse_rejects_unknown_missing_and_flag_value() {
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--rpc-url"]).is_err()); // missing value
        assert!(parse(&["--rpc-url", "--database-url", "x"]).is_err()); // flag swallowed
    }
}
