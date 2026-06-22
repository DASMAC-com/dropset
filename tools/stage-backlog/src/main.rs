//! `dropset-stage-backlog` — render the Dropset Linear Backlog as the
//! chips-only Task Staging dependency tree.
//!
//! This is the deterministic core of the `stage-backlog` skill: read the
//! project's open Backlog (with parents and declared blocking relations),
//! build the dependency tree from those edges plus file-overlap, and write
//! the rendered tree to the Task Staging document. The skill's agent step
//! still owns the prose-level work the binary can't do mechanically — merging
//! issues that belong in one PR, and placing legacy issues that predate the
//! `**Touches**:` field.
//!
//! Configuration comes entirely from the environment (no hard-coded ids,
//! never a committed token):
//!
//! * `LINEAR_API_KEY` — a personal API key (the headless binary can't use the
//!   OAuth MCP), sent as the `Authorization` header.
//! * `LINEAR_PROJECT_ID` — the Dropset project whose Backlog is staged.
//! * `LINEAR_TASK_STAGING_DOC_ID` — the document rewritten each run (not
//!   needed for `--dry-run`).
//!
//! Pass `--dry-run` to print the rendered tree to stdout without writing the
//! document.

mod linear;
mod model;
mod plan;

use anyhow::{Context, Result};

/// Read a required, non-empty environment variable.
fn env_var(name: &str) -> Result<String> {
    let value = std::env::var(name).with_context(|| format!("{name} is not set"))?;
    if value.trim().is_empty() {
        anyhow::bail!("{name} is empty");
    }
    Ok(value)
}

fn main() -> Result<()> {
    let mut dry_run = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "-h" | "--help" => {
                println!(
                    "Usage: dropset-stage-backlog [--dry-run]\n\n\
                     Renders the Dropset Linear Backlog as the Task Staging \
                     dependency tree.\n\
                     --dry-run  Print the tree to stdout; do not write the \
                     document."
                );
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other} (try --help)"),
        }
    }

    let client = linear::Client::new(env_var("LINEAR_API_KEY")?);
    let project_id = env_var("LINEAR_PROJECT_ID")?;

    let issues = client
        .fetch_backlog(&project_id)
        .context("reading the Backlog from Linear")?;

    for id in plan::missing_touches(&issues) {
        eprintln!(
            "warning: {id} has no **Touches**: field; placed by declared edges / \
             parent only — the skill's agent step should reconcile it"
        );
    }

    let document = plan::render(&issues);

    if dry_run {
        print!("{document}");
        eprintln!("stage-backlog (dry-run) | {} backlog issues", issues.len());
        return Ok(());
    }

    let doc_id = env_var("LINEAR_TASK_STAGING_DOC_ID")?;
    client
        .save_document(&doc_id, &document)
        .context("writing the Task Staging document")?;
    println!(
        "stage-backlog | {} backlog issues | staging document updated",
        issues.len()
    );
    Ok(())
}
