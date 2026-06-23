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
//!
//! The `merge` subcommand is the deterministic half of the same-PR issue
//! merge: given a group of `ENG-###` ids the skill judged to belong in one PR,
//! it folds them onto the lowest-numbered canonical (write-before-close) and
//! closes the rest as duplicates. See [`merge`].

mod linear;
mod merge;
mod model;
mod plan;

use anyhow::{Context, Result};

const HELP: &str = "\
Usage:
  dropset-stage-backlog [--dry-run]
      Render the Dropset Linear Backlog as the Task Staging dependency tree.
      --dry-run  Print the tree to stdout; do not write the document.

  dropset-stage-backlog merge [--dry-run] ENG-A,ENG-B[,ENG-C] [<group> ...]
      Merge each comma-separated group of issues onto its lowest-numbered
      canonical (union the descriptions, fingerprints, touches, and blocking
      edges; close the others as duplicates), write-before-close.
      --dry-run  Print the computed fold for each group; perform no writes.";

/// Read a required, non-empty environment variable.
fn env_var(name: &str) -> Result<String> {
    let value = std::env::var(name).with_context(|| format!("{name} is not set"))?;
    if value.trim().is_empty() {
        anyhow::bail!("{name} is empty");
    }
    Ok(value)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("merge") {
        return run_merge(&args[1..]);
    }
    run_render(&args)
}

/// Default mode: render the whole Backlog tree to the Task Staging document.
fn run_render(args: &[String]) -> Result<()> {
    let mut dry_run = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
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

/// `merge` mode: fold each comma-separated group onto its canonical and close
/// the rest as duplicates. The grouping itself is the skill's prose judgment;
/// the binary owns the mechanical fold.
fn run_merge(args: &[String]) -> Result<()> {
    let mut dry_run = false;
    let mut groups: Vec<Vec<String>> = Vec::new();
    for arg in args {
        if arg == "--dry-run" {
            dry_run = true;
            continue;
        }
        if arg.starts_with('-') {
            anyhow::bail!("unknown argument: {arg} (try --help)");
        }
        let ids: Vec<String> = arg
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if ids.len() < 2 {
            anyhow::bail!("merge group `{arg}` needs at least two comma-separated issue ids");
        }
        groups.push(ids);
    }
    if groups.is_empty() {
        anyhow::bail!("merge needs at least one comma-separated group of issue ids");
    }

    let client = linear::Client::new(env_var("LINEAR_API_KEY")?);

    for ids in &groups {
        let (members, team_id) = client
            .fetch_merge_group(ids)
            .with_context(|| format!("fetching merge group {}", ids.join(",")))?;
        let plan = merge::plan_merge(&members)?;

        if dry_run {
            print_merge_plan(&plan);
            continue;
        }

        execute_merge(&client, &plan, &team_id)
            .with_context(|| format!("merging onto {}", plan.canonical_id))?;
        println!(
            "stage-backlog merge | {} ← {} | {} closed as duplicates",
            plan.canonical_id,
            ids.join(", "),
            plan.duplicates.len()
        );
    }
    Ok(())
}

/// Perform a single group's writes, write-before-close: fold the canonical
/// (description, priority, union edges) and confirm it, then close each member.
///
/// Re-running after a *clean* failure is safe: the fold is an idempotent
/// overwrite, and the union edges self-heal because [`fetch_merge_group`] re-reads
/// the canonical's live relations each run, so `*_to_add` excludes edges a prior
/// partial run already created. The gap is the close loop: a failure *between*
/// members can leave the group half-closed, and a re-run re-attempts the
/// `duplicate` relation on an already-closed member — which errors if Linear
/// rejects a duplicate `issueRelationCreate`. Making the close loop fully
/// idempotent needs Linear's exact duplicate-relation semantics, which can't be
/// verified here; until then, finish a half-closed group by hand.
fn execute_merge(client: &linear::Client, plan: &merge::MergePlan, team_id: &str) -> Result<()> {
    // 1. Fold everything onto the canonical and confirm before any close — if
    //    interrupted here, every member still exists and holds its own state.
    client
        .update_fold(&plan.canonical_uuid, &plan.description, plan.priority)
        .context("writing the canonical fold")?;
    for blocker in &plan.blocked_by_to_add {
        let uuid = client.resolve_uuid(blocker)?;
        client.create_relation(&uuid, &plan.canonical_uuid, "blocks")?;
    }
    for blocked in &plan.blocks_to_add {
        let uuid = client.resolve_uuid(blocked)?;
        client.create_relation(&plan.canonical_uuid, &uuid, "blocks")?;
    }

    // 2. Only now close the members as duplicates of the canonical.
    let duplicate_state = client.duplicate_state_id(team_id)?;
    for (id, uuid) in &plan.duplicates {
        client
            .create_relation(uuid, &plan.canonical_uuid, "duplicate")
            .with_context(|| format!("marking {id} a duplicate of {}", plan.canonical_id))?;
        client
            .set_state(uuid, &duplicate_state)
            .with_context(|| format!("closing {id}"))?;
    }
    Ok(())
}

/// Print the computed fold for `--dry-run`, so the operator can review it
/// before any write.
fn print_merge_plan(plan: &merge::MergePlan) {
    println!(
        "canonical: {} (priority {})",
        plan.canonical_id, plan.priority
    );
    let duplicate_ids: Vec<&str> = plan.duplicates.iter().map(|(id, _)| id.as_str()).collect();
    println!("close as duplicates: {}", duplicate_ids.join(", "));
    if !plan.blocked_by_to_add.is_empty() {
        println!("add blockedBy: {}", plan.blocked_by_to_add.join(", "));
    }
    if !plan.blocks_to_add.is_empty() {
        println!("add blocks: {}", plan.blocks_to_add.join(", "));
    }
    println!("--- folded description ---\n{}\n---", plan.description);
}
