//! The ceremony record: an append-only log of every write a mainnet session
//! makes, kept by the operator **outside the repo**.
//!
//! One line per event — the step, the commit the binary was built from, and
//! whatever the step produced (addresses, the deployed binary's sha256, each
//! transaction signature) — as tab-separated `key=value` pairs, so it greps
//! and diffs without a parser. It is opened at launch, before the first frame,
//! so a log that cannot be written refuses the session rather than discovering
//! it after a transaction has landed unrecorded.
//!
//! Signatures reach it without each step having to remember: a job's
//! [`crate::job::Logger`] carries the log, and [`crate::chain::send_logged`]
//! records every signature it reports.

use crate::cluster::Cluster;
use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment variable overriding the ceremony log's path.
pub const CEREMONY_LOG_ENV: &str = "DROPSET_CEREMONY_LOG";

/// An open ceremony log.
pub struct CeremonyLog {
    path: PathBuf,
    file: Mutex<File>,
    cluster: Cluster,
    commit: String,
}

impl CeremonyLog {
    /// Open (creating if needed) the log at `path` for appending.
    ///
    /// Refuses a path inside `repo_root`: the record holds addresses and
    /// signatures that belong to the operator, and a file in the working tree
    /// is one `git add -A` from being committed.
    pub fn open(path: &Path, repo_root: &Path, cluster: Cluster) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create ceremony log dir {}", parent.display()))?;
        let dir = parent
            .canonicalize()
            .with_context(|| format!("resolve {}", parent.display()))?;
        let root = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.to_path_buf());
        if dir.starts_with(&root) {
            bail!(
                "the ceremony log {} is inside the repo — keep it outside the working tree \
                 (set {CEREMONY_LOG_ENV})",
                path.display()
            );
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open ceremony log {}", path.display()))?;
        let commit = match git_head(repo_root) {
            Ok(head) if head.dirty => format!("{}+dirty", head.commit),
            Ok(head) => head.commit,
            Err(_) => "unknown".to_string(),
        };
        let log = Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            cluster,
            commit,
        };
        log.append("session", &[])?;
        Ok(log)
    }

    /// Where the log lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one `step` line carrying `fields`, flushed to disk before
    /// returning.
    pub fn append(&self, step: &str, fields: &[(&str, &str)]) -> Result<()> {
        let line = format_line(unix_now(), self.cluster, &self.commit, step, fields);
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        file.write_all(line.as_bytes())
            .and_then(|()| file.sync_data())
            .with_context(|| format!("append to ceremony log {}", self.path.display()))
    }
}

/// The default log path for `cluster`: [`CEREMONY_LOG_ENV`] when set, else
/// `~/.dropset/ceremony-<cluster>.log`.
pub fn default_path(cluster: Cluster) -> Option<PathBuf> {
    if let Some(path) = std::env::var(CEREMONY_LOG_ENV)
        .ok()
        .filter(|p| !p.trim().is_empty())
    {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(format!(".dropset/ceremony-{}.log", cluster.label())))
}

/// The commit `repo_root` is checked out at.
pub struct GitHead {
    pub commit: String,
    /// Tracked files differ from the commit, so the commit alone does not
    /// describe what was built.
    pub dirty: bool,
}

/// Read `repo_root`'s `HEAD` and whether its tracked files are modified.
pub fn git_head(repo_root: &Path) -> Result<GitHead> {
    let git = |args: &[&str]| -> Result<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(args)
            .output()
            .context("run git")?;
        if !out.status.success() {
            bail!("git {} failed", args.join(" "));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    Ok(GitHead {
        commit: git(&["rev-parse", "HEAD"])?,
        dirty: !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty(),
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One log line. Tabs and newlines in a value are flattened to spaces so a
/// value can never forge a field or a line.
fn format_line(
    ts: u64,
    cluster: Cluster,
    commit: &str,
    step: &str,
    fields: &[(&str, &str)],
) -> String {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut line = format!(
        "ts={ts}\tcluster={}\tcommit={}\tstep={}",
        cluster.label(),
        clean(commit),
        clean(step)
    );
    for (key, value) in fields {
        line.push_str(&format!("\t{}={}", clean(key), clean(value)));
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn lines_are_one_record_each_and_cannot_be_forged() {
        let line = format_line(
            7,
            Cluster::Mainnet,
            "abc",
            "init",
            &[("signature", "sig\tstep=forged\nts=0")],
        );
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(line.matches('\t').count(), 4);
        assert!(line.starts_with("ts=7\tcluster="), "{line}");
    }

    #[test]
    fn refuses_a_log_inside_the_repo_and_appends_outside_it() {
        let root = repo_root();
        let inside = root.join("target/ceremony-test.log");
        assert!(CeremonyLog::open(&inside, &root, Cluster::Localnet).is_err());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/ceremony.log");
        let log = CeremonyLog::open(&path, &root, Cluster::Localnet).unwrap();
        log.append("publish", &[("sha256", "ff")]).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "{body}");
        assert!(lines[0].contains("step=session"));
        assert!(lines[1].contains("step=publish") && lines[1].ends_with("sha256=ff"));
    }
}
