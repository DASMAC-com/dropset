//! `dropset-session-metrics` — account for where a Claude Code session spent
//! its tokens, so the `session-metrics` skill can recommend concrete trims.
//!
//! Given a `--session-id`, the binary resolves the session's on-disk transcript
//! itself, reads it (and its sub-agent transcripts) in its **own** process — so
//! the multi-megabyte file never enters the model's context — and prints a
//! compact, ranked summary: session-wide token totals, a cache-hit rate, the
//! tools whose results cost the most, the single largest results, and a
//! per-sub-agent rollup. Pass `--json` for the same data as JSON.
//!
//! Nothing about the host is hard-coded. The Claude home is read from
//! `CLAUDE_CONFIG_DIR` (falling back to `~/.claude`), and the per-project
//! transcript directory is derived from the working directory the same way
//! Claude Code slugs it — with a scan of every project directory as a fallback,
//! so a worktree whose slug doesn't match still resolves by session id.

mod model;

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use model::{Report, SessionAggregator};

const HELP: &str = "\
Usage:
  dropset-session-metrics --session-id <uuid> [--json]
      Summarize where a session's tokens went: totals, cache-hit rate,
      the costliest tools, the largest single results, and a per-sub-agent
      rollup. Reads the transcript in this process; only the summary is
      printed.
      --json  Emit the summary as JSON instead of Markdown.";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return Ok(());
    }

    let mut session_id: Option<String> = None;
    let mut as_json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => as_json = true,
            "--session-id" => {
                session_id = Some(
                    iter.next()
                        .cloned()
                        .ok_or_else(|| anyhow!("--session-id needs a value"))?,
                );
            }
            other => anyhow::bail!("unknown argument: {other} (try --help)"),
        }
    }
    let session_id = session_id.ok_or_else(|| anyhow!("--session-id is required (try --help)"))?;

    let transcript = resolve_transcript(&session_id)
        .with_context(|| format!("locating the transcript for session {session_id}"))?;
    let report = aggregate(&transcript, &session_id)
        .with_context(|| format!("reading {}", transcript.display()))?;

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("serializing the report")?
        );
    } else {
        print!("{}", report.to_markdown(short_id(&session_id)));
    }
    Ok(())
}

/// The Claude home directory: `CLAUDE_CONFIG_DIR` if set, else `~/.claude`.
fn claude_home() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = std::env::var("HOME").context("neither CLAUDE_CONFIG_DIR nor HOME is set")?;
    Ok(PathBuf::from(home).join(".claude"))
}

/// Claude Code names each project's transcript directory after the working
/// directory, replacing every `/` and `.` with `-`.
fn slugify(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// Resolve a session id to its transcript file. Tries the slug of the current
/// working directory first (the common case), then falls back to scanning every
/// project directory for `<session-id>.jsonl` — so a worktree whose slug differs
/// from the cwd still resolves.
fn resolve_transcript(session_id: &str) -> Result<PathBuf> {
    let projects = claude_home()?.join("projects");
    let file_name = format!("{session_id}.jsonl");

    if let Ok(cwd) = std::env::current_dir() {
        let primary = projects.join(slugify(&cwd)).join(&file_name);
        if primary.is_file() {
            return Ok(primary);
        }
    }

    let entries = fs::read_dir(&projects)
        .with_context(|| format!("reading projects directory {}", projects.display()))?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(&file_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "no transcript {file_name} found under {}",
        projects.display()
    ))
}

/// Stream the main transcript and every sub-agent transcript into a [`Report`].
fn aggregate(transcript: &Path, session_id: &str) -> Result<Report> {
    let mut agg = SessionAggregator::new();
    for line in read_lines(transcript)? {
        agg.ingest_main_line(&line);
    }

    // Sub-agent transcripts live in `<transcript-dir>/<session-id>/subagents/`.
    let subagents_dir = transcript
        .parent()
        .map(|dir| dir.join(session_id).join("subagents"));
    if let Some(dir) = subagents_dir {
        if dir.is_dir() {
            for entry in fs::read_dir(&dir)
                .with_context(|| format!("reading sub-agent directory {}", dir.display()))?
                .flatten()
            {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let label = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("agent")
                    .to_string();
                for line in read_lines(&path)? {
                    agg.ingest_subagent_line(&label, &line);
                }
            }
        }
    }

    Ok(agg.finish())
}

/// Read a file's lines, skipping any that can't be decoded (a partial trailing
/// write, say) so accounting never aborts on a malformed tail.
fn read_lines(path: &Path) -> Result<impl Iterator<Item = String>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(BufReader::new(file).lines().map_while(Result::ok))
}

/// The first segment of a UUID — enough to identify the session in the summary
/// without printing the whole id.
fn short_id(session_id: &str) -> &str {
    session_id.split('-').next().unwrap_or(session_id)
}
