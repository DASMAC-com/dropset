//! The agent-shell refusal shared by the cockpit's mainnet mode and the
//! headless teardown on a non-localnet endpoint.
//!
//! Claude Code sets `CLAUDECODE=1` in every tool shell it opens, sub-agents
//! included, and an operator's own terminal never carries it — so its
//! presence is the mark of an agent's shell. Mainnet operations are the
//! operator's alone, run from their own terminal; the refusal says so in
//! plain words rather than exiting with a bare code. The `tui-mainnet`
//! Makefile target prints the same text before `cargo run`, so it appears
//! even when the binary is not built — keep the two in step.

use anyhow::{bail, Result};
use std::ffi::OsString;

/// The variable Claude Code sets in every tool shell it opens.
pub const AGENT_SHELL_VAR: &str = "CLAUDECODE";

/// The refusal, stating the rule. Mirrored verbatim by the Makefile's
/// `tui-mainnet` guard.
pub const REFUSAL: &str = "Refused: CLAUDECODE is set, which marks this as an agent's shell \
     (Claude Code sets it in every shell it opens). An agent shell, sub-agents included, \
     never runs mainnet operations. Do not run this from a sub-agent or an agent session: \
     the operator runs the cockpit in their own terminal.";

/// Refuse when this process runs in an agent's shell.
pub fn refuse_agent_shell() -> Result<()> {
    refuse_if_marked(std::env::var_os(AGENT_SHELL_VAR))
}

/// Refuse on a non-empty value — the same value test the Makefile guard
/// applies, since make publishes an unset variable as an empty one.
fn refuse_if_marked(value: Option<OsString>) -> Result<()> {
    match value {
        Some(v) if !v.is_empty() => bail!("{REFUSAL}"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_marked_shell_with_the_rule() {
        let err = refuse_if_marked(Some("1".into())).unwrap_err().to_string();
        assert!(err.contains(AGENT_SHELL_VAR), "names the variable: {err}");
        assert!(
            err.contains("sub-agents included"),
            "covers sub-agents: {err}"
        );
        assert!(
            err.contains("never runs mainnet operations"),
            "states the rule: {err}"
        );
        assert!(
            err.contains("their own terminal"),
            "says who runs it: {err}"
        );
    }

    #[test]
    fn allows_an_unmarked_shell() {
        assert!(refuse_if_marked(None).is_ok());
        assert!(refuse_if_marked(Some("".into())).is_ok());
    }
}
