//! Where a mainnet session's JSON-RPC endpoint comes from: a 1Password
//! reference the binary reads itself, never a typed URL.
//!
//! A paid provider's endpoint carries its API key in the URL, so a typed
//! export puts the key in shell history, scrollback and the environment of
//! every later process in that tab. [`MAINNET_RPC_ENV`] therefore holds a
//! **pointer**, an `op://<vault>/<item>/<field>` reference, and the binary
//! runs `op read` on it as a direct subprocess, then `op signout`, before the
//! typed confirmation and the first frame. The resolved URL is held in memory
//! and handed to the RPC client; it is never placed in the process
//! environment, never written to a file and never printed.
//!
//! The variable is treated four ways ([`resolve`]):
//!
//! - an `op://` reference is dereferenced;
//! - a loopback URL is admitted untouched, for the dress rehearsal, whose own
//!   checks (`Cluster::verify_genesis`) apply unchanged;
//! - the exact keyless public endpoint ([`PUBLIC_MAINNET_RPC`]) is admitted
//!   only in a session that can spend nothing (the watch-only demo);
//! - anything else, a literal `https://` URL included, is refused without
//!   being echoed.
//!
//! There is one endpoint from one reference: no secondary variable, and no
//! fallback to the public endpoint when a read fails. The operator's guide is
//! [`RPC_DOC`].

use crate::cluster::{is_rehearsal_loopback, MAINNET_RPC_ENV};
use anyhow::{anyhow, bail, Result};
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Environment variable naming the 1Password account `op` reads from. The
/// machine is signed in to more than one account, and `op` fails at client
/// init without `--account` there; unset, the flag is omitted.
pub const OP_ACCOUNT_ENV: &str = "DROPSET_OP_ACCOUNT";

/// The keyless public mainnet-beta endpoint — admitted only for a session
/// that can spend nothing, since it carries no secret to leak. Mirrors the
/// Makefile's `PUBLIC_MAINNET_RPC`.
pub const PUBLIC_MAINNET_RPC: &str = "https://api.mainnet-beta.solana.com";

/// Where the operator reads how the variable is set.
pub const RPC_DOC: &str =
    "docs/conventions/local-integrations.md, \"The cockpit's mainnet RPC URL\"";

/// The prefix that marks a 1Password secret reference.
const REFERENCE_PREFIX: &str = "op://";

/// How long one `op` call may run. Long enough for the operator to answer the
/// CLI's authorization prompt, short enough that a wedged `op` refuses the
/// launch rather than hanging it.
pub const OP_TIMEOUT: Duration = Duration::from_secs(120);

/// What one `op` invocation returned. Its stderr is not captured: it goes
/// straight to the terminal, so `op`'s own error is what the operator sees.
pub struct OpOutput {
    /// Whether `op` exited zero.
    pub success: bool,
    /// What `op` printed on stdout — for `op read`, the secret.
    pub stdout: Vec<u8>,
}

/// The values [`resolve`] decides from, read from the environment by
/// [`Inputs::from_env`] so the decision itself is testable without one.
pub struct Inputs {
    /// [`MAINNET_RPC_ENV`]'s value, if set.
    pub value: Option<String>,
    /// [`OP_ACCOUNT_ENV`]'s value, if set and non-empty.
    pub account: Option<String>,
    /// Whether this session can spend nothing: the ephemeral wallet and no
    /// leader, quote or program key. Only then is [`PUBLIC_MAINNET_RPC`]
    /// admitted.
    pub spends_nothing: bool,
}

impl Inputs {
    /// Read both variables from this process's environment.
    pub fn from_env(spends_nothing: bool) -> Self {
        let non_empty = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Self {
            value: non_empty(MAINNET_RPC_ENV),
            account: non_empty(OP_ACCOUNT_ENV),
            spends_nothing,
        }
    }
}

/// Resolve the mainnet endpoint, refusing every value that is not safe.
///
/// `refuse_agent_shell` runs **first**, before the value is even looked at,
/// so an agent's shell can never raise a 1Password prompt. `op` runs one
/// `op` invocation (the real one is [`run_op`]); `warn` prints a
/// non-fatal line.
pub fn resolve(
    inputs: &Inputs,
    refuse_agent_shell: impl FnOnce() -> Result<()>,
    mut op: impl FnMut(&[&str]) -> io::Result<OpOutput>,
    mut warn: impl FnMut(&str),
) -> Result<String> {
    refuse_agent_shell()?;
    let Some(value) = inputs.value.as_deref() else {
        bail!(
            "mainnet mode needs {MAINNET_RPC_ENV} set to a 1Password reference \
             (op://<vault>/<item>/<field>); there is deliberately no default — see {RPC_DOC}"
        );
    };
    if value.starts_with(REFERENCE_PREFIX) {
        return dereference(value, inputs.account.as_deref(), &mut op, &mut warn);
    }
    if is_rehearsal_loopback(value) {
        return Ok(value.to_string());
    }
    if value == PUBLIC_MAINNET_RPC {
        if inputs.spends_nothing {
            return Ok(value.to_string());
        }
        bail!(
            "{MAINNET_RPC_ENV} is the public endpoint, which is admitted only for a watch-only \
             session (--wallet ephemeral, no leader, quote or program key). A session that can \
             sign takes a 1Password reference — see {RPC_DOC}"
        );
    }
    // The value is never echoed: a stale typed export is exactly the keyed URL.
    bail!(
        "{MAINNET_RPC_ENV} holds a literal value, refused without printing it: a mainnet \
         endpoint is never typed. Set the variable to a 1Password reference \
         (op://<vault>/<item>/<field>) — see {RPC_DOC}. Only a loopback URL, for the \
         rehearsal, is admitted as a literal."
    )
}

/// Append `--account <account>` to `args` when an account is named.
fn with_account<'a>(mut args: Vec<&'a str>, account: Option<&'a str>) -> Vec<&'a str> {
    if let Some(account) = account {
        args.extend(["--account", account]);
    }
    args
}

/// `op read` the reference, sign out, then admit only an `https://` result.
fn dereference(
    reference: &str,
    account: Option<&str>,
    op: &mut impl FnMut(&[&str]) -> io::Result<OpOutput>,
    warn: &mut impl FnMut(&str),
) -> Result<String> {
    let read = op(&with_account(vec!["read", reference], account));
    // Sign out whenever `op` ran at all, so a failed read leaves no
    // authorization behind either. Only an `op` that could not start is
    // skipped — there is nothing to sign out of.
    let spawned = !matches!(&read, Err(e) if e.kind() == io::ErrorKind::NotFound);
    if spawned {
        match op(&with_account(vec!["signout"], account)) {
            Ok(out) if out.success => {}
            _ => warn(
                "warning: `op signout` failed — the CLI's authorization idles out on its own \
                 in ten minutes",
            ),
        }
    }
    let unread = |why: String| {
        anyhow!(
            "{why}; could not read {MAINNET_RPC_ENV}'s 1Password reference — refusing the \
             launch (there is no public fallback)"
        )
    };
    let out = read.map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => unread("`op` is not installed or not on PATH".into()),
        io::ErrorKind::TimedOut => unread(format!(
            "`op read` timed out after {} s",
            OP_TIMEOUT.as_secs()
        )),
        _ => unread(format!("`op read` could not run: {e}")),
    })?;
    if !out.success {
        // `op`'s own error has already gone to the terminal.
        return Err(unread("`op read` failed (its error is above)".into()));
    }
    let url = String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if url.is_empty() {
        bail!("{MAINNET_RPC_ENV}'s 1Password reference resolved to an empty value — refused");
    }
    if !url.starts_with("https://") {
        bail!(
            "{MAINNET_RPC_ENV}'s 1Password reference resolved to something that is not an \
             https:// URL — refused, and the value is not printed"
        );
    }
    Ok(url)
}

/// Run `op` with `args` as a direct subprocess: fixed arguments, no shell, and
/// [`OP_TIMEOUT`] as the bound. Stdin and stderr are the terminal's, so the
/// CLI's prompt and its errors reach the operator; stdout is captured, since
/// for `op read` it is the secret.
pub fn run_op(args: &[&str]) -> io::Result<OpOutput> {
    let mut child = Command::new("op")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    // Drained on its own thread so a large write cannot block `op` on a full
    // pipe while this thread waits for it to exit.
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let deadline = Instant::now() + OP_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "op timed out"));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let stdout = reader
        .join()
        .map_err(|_| io::Error::other("the op stdout reader panicked"))??;
    Ok(OpOutput {
        success: status.success(),
        stdout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = "op://Vault/Item/url";
    const KEYED: &str = "https://mainnet.example-rpc.com/?api-key=SUPERSECRET";

    fn inputs(value: Option<&str>, account: Option<&str>, spends_nothing: bool) -> Inputs {
        Inputs {
            value: value.map(str::to_string),
            account: account.map(str::to_string),
            spends_nothing,
        }
    }

    /// Run [`resolve`] over a scripted `op`: `read` answers the read, and the
    /// signout succeeds unless `signout_ok` is false. Returns the result, every
    /// `op` argument list, and every warning.
    fn run(
        inputs: &Inputs,
        read: impl Fn() -> io::Result<OpOutput>,
        signout_ok: bool,
    ) -> (Result<String>, Vec<Vec<String>>, Vec<String>) {
        let mut calls = Vec::new();
        let mut warnings = Vec::new();
        let result = resolve(
            inputs,
            || Ok(()),
            |args: &[&str]| {
                calls.push(args.iter().map(|a| a.to_string()).collect());
                if args[0] == "read" {
                    read()
                } else {
                    Ok(OpOutput {
                        success: signout_ok,
                        stdout: Vec::new(),
                    })
                }
            },
            |line: &str| warnings.push(line.to_string()),
        );
        (result, calls, warnings)
    }

    fn printed(url: &str) -> impl Fn() -> io::Result<OpOutput> + '_ {
        move || {
            Ok(OpOutput {
                success: true,
                stdout: format!("{url}\n").into_bytes(),
            })
        }
    }

    fn no_op() -> io::Result<OpOutput> {
        panic!("op must not run")
    }

    #[test]
    fn a_reference_is_read_then_signed_out_with_the_account() {
        let (url, calls, warnings) = run(
            &inputs(Some(REFERENCE), Some("acct"), false),
            printed(KEYED),
            true,
        );
        assert_eq!(url.unwrap(), KEYED);
        assert_eq!(
            calls,
            vec![
                vec!["read", REFERENCE, "--account", "acct"],
                vec!["signout", "--account", "acct"],
            ]
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn an_unset_account_omits_the_flag() {
        let (url, calls, _) = run(&inputs(Some(REFERENCE), None, false), printed(KEYED), true);
        assert_eq!(url.unwrap(), KEYED);
        assert_eq!(calls, vec![vec!["read", REFERENCE], vec!["signout"]]);
    }

    #[test]
    fn a_failed_signout_is_a_warning_not_a_refusal() {
        let (url, _, warnings) = run(&inputs(Some(REFERENCE), None, false), printed(KEYED), false);
        assert_eq!(url.unwrap(), KEYED);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("op signout"), "{warnings:?}");
    }

    #[test]
    fn the_agent_shell_refusal_runs_before_anything_else() {
        // Even a value that would otherwise be read: `op` must never run.
        for value in [Some(REFERENCE), Some(KEYED), None] {
            let err = resolve(
                &inputs(value, None, false),
                || bail!("agent shell"),
                |_: &[&str]| no_op(),
                |_: &str| panic!("nothing to warn about"),
            )
            .unwrap_err();
            assert_eq!(err.to_string(), "agent shell");
        }
    }

    #[test]
    fn a_literal_url_is_refused_without_being_echoed() {
        for value in [
            KEYED,
            "http://mainnet.example-rpc.com/SUPERSECRET",
            "https://api.mainnet-beta.solana.com/",
            "SUPERSECRET",
        ] {
            let (result, calls, _) = run(&inputs(Some(value), None, true), no_op, true);
            let err = format!("{:#}", result.unwrap_err());
            assert!(err.contains("never typed"), "{value}: {err}");
            assert!(
                err.contains("local-integrations.md"),
                "names the doc: {err}"
            );
            assert!(!err.contains("SUPERSECRET"), "echoed the value: {err}");
            assert!(calls.is_empty(), "1Password is not touched for a literal");
        }
    }

    #[test]
    fn a_loopback_url_runs_the_rehearsal_without_1password() {
        for value in ["http://127.0.0.1:8899", "http://localhost:8899/"] {
            let (url, calls, _) = run(&inputs(Some(value), None, false), no_op, true);
            assert_eq!(url.unwrap(), value);
            assert!(calls.is_empty());
        }
    }

    #[test]
    fn the_public_endpoint_is_admitted_only_when_nothing_can_spend() {
        let watch = inputs(Some(PUBLIC_MAINNET_RPC), None, true);
        assert_eq!(run(&watch, no_op, true).0.unwrap(), PUBLIC_MAINNET_RPC);
        let funded = inputs(Some(PUBLIC_MAINNET_RPC), None, false);
        let err = format!("{:#}", run(&funded, no_op, true).0.unwrap_err());
        assert!(err.contains("watch-only"), "{err}");
    }

    #[test]
    fn an_unset_variable_is_refused_with_no_default() {
        let (result, calls, _) = run(&inputs(None, None, true), no_op, true);
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains(MAINNET_RPC_ENV) && err.contains("no default"),
            "{err}"
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn every_op_failure_refuses_with_no_public_fallback() {
        type Read = Box<dyn Fn() -> io::Result<OpOutput>>;
        let failures: [(&str, Read); 4] = [
            (
                "exit",
                Box::new(|| {
                    Ok(OpOutput {
                        success: false,
                        stdout: Vec::new(),
                    })
                }),
            ),
            ("timeout", Box::new(|| Err(io::ErrorKind::TimedOut.into()))),
            ("missing", Box::new(|| Err(io::ErrorKind::NotFound.into()))),
            (
                "other",
                Box::new(|| Err(io::ErrorKind::PermissionDenied.into())),
            ),
        ];
        for (name, read) in failures {
            let (result, calls, _) = run(&inputs(Some(REFERENCE), None, true), read, true);
            let err = format!("{:#}", result.unwrap_err());
            assert!(
                err.contains(MAINNET_RPC_ENV),
                "{name}: names the variable: {err}"
            );
            assert!(err.contains("no public fallback"), "{name}: {err}");
            // An `op` that never started has nothing to sign out of.
            let signed_out = calls.iter().any(|c| c[0] == "signout");
            assert_eq!(signed_out, name != "missing", "{name}: {calls:?}");
        }
    }

    /// `make mainnet-demo` passes its own copy of the public endpoint, which
    /// must be the exact string admitted here.
    #[test]
    fn the_makefile_demo_passes_the_admitted_public_endpoint() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../Makefile");
        let makefile = std::fs::read_to_string(path).unwrap();
        let line = makefile
            .lines()
            .find_map(|l| l.strip_prefix("PUBLIC_MAINNET_RPC := "))
            .expect("the Makefile defines PUBLIC_MAINNET_RPC");
        assert_eq!(line.trim(), PUBLIC_MAINNET_RPC);
    }

    #[test]
    fn a_non_https_or_empty_result_is_refused_without_being_printed() {
        for value in ["", "   ", "http://SUPERSECRET.example.com", "SUPERSECRET"] {
            let (result, _, _) = run(&inputs(Some(REFERENCE), None, false), printed(value), true);
            let err = format!("{:#}", result.unwrap_err());
            assert!(err.contains("refused"), "{value:?}: {err}");
            assert!(!err.contains("SUPERSECRET"), "printed the value: {err}");
        }
    }
}
