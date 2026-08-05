//! Wall-clock bookkeeping of the last *live* reference price the bot stamped,
//! persisted across restarts.
//!
//! Everything else in the bot's state is chain-derived and re-read each tick
//! (`context` module header). This one fact is not recoverable from a single
//! read: the vault stores its reference price and the `quote_slot` it was
//! stamped at, but not *when* in wall-clock terms — and slot arithmetic is
//! exactly what a chain halt makes unusable, since slots stop ticking while the
//! resting levels stay live. So the bot writes down its own timestamp and reads
//! it back on startup to tell a book it refreshed seconds ago from one that has
//! been resting, unattended, for an unbounded stretch.
//!
//! **One file per market, not one file for the run.** The demo drives one bot
//! process per market (the TUI starts and stops each independently, passing a
//! single `--market`), so several processes write this state concurrently. A
//! shared file would have them clobber each other's entries on every write;
//! keyed by market address, each process only ever touches its own. Each write
//! goes to a temporary sibling and is renamed into place, so a reader never
//! observes a half-written record and a crash mid-write leaves the previous one.
//!
//! A missing, unreadable, or nonsensical record reads as `None` — "unknown" —
//! which `model::invalidate::should_invalidate` treats as stale. Every failure
//! mode here therefore lands on the safe side; nothing in this module returns an
//! error that should stop a tick.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default directory for the persisted per-market records, relative to the
/// process's working directory (the repo root, as the Makefile targets and the
/// TUI both launch the bot from there). Git-ignored.
pub const DEFAULT_STATE_DIR: &str = ".maker-bot/quote-state";

/// One market's persisted record. `symbol` and `market` are written for the
/// benefit of whoever reads the directory by hand — only `last_live_stamp_unix`
/// is load-bearing.
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    symbol: String,
    market: String,
    /// Unix seconds at which the bot last stamped a live (non-kill) reference
    /// price on this market.
    last_live_stamp_unix: u64,
}

/// The state directory. Cheap to clone (it is a path); hand each market its own
/// [`QuoteState`] handle with [`QuoteStateStore::for_market`].
#[derive(Clone, Debug)]
pub struct QuoteStateStore {
    dir: PathBuf,
}

impl QuoteStateStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// A handle scoped to one market. Creating it touches no filesystem — the
    /// directory is created lazily on the first successful write, so a run that
    /// never stamps leaves no trace.
    pub fn for_market(&self, market: Pubkey, symbol: &str) -> QuoteState {
        QuoteState {
            path: self.dir.join(format!("{market}.json")),
            dir: self.dir.clone(),
            market,
            symbol: symbol.to_string(),
        }
    }
}

impl Default for QuoteStateStore {
    fn default() -> Self {
        Self::new(DEFAULT_STATE_DIR)
    }
}

/// One market's handle onto the persisted last-live-stamp record.
#[derive(Clone, Debug)]
pub struct QuoteState {
    path: PathBuf,
    dir: PathBuf,
    market: Pubkey,
    symbol: String,
}

impl QuoteState {
    /// How long ago the bot last stamped a live reference on this market, or
    /// `None` when that can't be established. Every unreadable state — no file,
    /// unparseable JSON, a stamp in the future (the clock moved backwards, so
    /// the record can't be trusted as an *upper* bound on age) — reads as
    /// `None`, which the caller treats as stale.
    pub fn age(&self, now: SystemTime) -> Option<Duration> {
        let bytes = std::fs::read(&self.path).ok()?;
        let record: Record = serde_json::from_slice(&bytes).ok()?;
        let now_unix = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
        now_unix
            .checked_sub(record.last_live_stamp_unix)
            .map(Duration::from_secs)
    }

    /// Record `now` as the moment a live reference price landed on this market.
    ///
    /// Called only after a *live* stamp — never after the kill stamp, so the
    /// record keeps meaning "when this book was last correctly priced". The kill
    /// stamp needs no record: it leaves the reference invalid, which the
    /// on-chain read alone is enough to recognize.
    pub fn record(&self, now: SystemTime) -> Result<()> {
        let last_live_stamp_unix = now
            .duration_since(UNIX_EPOCH)
            .context("wall clock before the Unix epoch")?
            .as_secs();
        let record = Record {
            symbol: self.symbol.clone(),
            market: self.market.to_string(),
            last_live_stamp_unix,
        };
        let json = serde_json::to_vec_pretty(&record).context("encode quote state")?;
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("create {}", self.dir.display()))?;
        write_atomically(&self.path, &json)
    }
}

/// Write `bytes` to `path` via a temporary sibling and a rename, so a concurrent
/// reader sees either the previous record or the new one — never a partial file.
/// The temporary name carries the target's file name, so the per-market
/// isolation holds for it too.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.to_path_buf();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    tmp.set_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename into {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory under the target dir, named per test so the cases
    /// don't collide. Removed and recreated on entry rather than on exit, so a
    /// failing test leaves its files behind to inspect.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dropset-quote-state-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn state(dir: &Path) -> QuoteState {
        QuoteStateStore::new(dir).for_market(Pubkey::new_unique(), "EURC")
    }

    #[test]
    fn no_record_reads_as_unknown() {
        let dir = scratch("missing");
        assert_eq!(state(&dir).age(SystemTime::now()), None);
    }

    #[test]
    fn a_recorded_stamp_round_trips_to_an_age() {
        let dir = scratch("round-trip");
        let st = state(&dir);
        let then = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        st.record(then).unwrap();
        // Read back 90 s later: the age is the gap between the two clocks.
        let age = st.age(then + Duration::from_secs(90)).unwrap();
        assert_eq!(age, Duration::from_secs(90));
        // And at the instant it was written, the quote is brand new.
        assert_eq!(st.age(then).unwrap(), Duration::ZERO);
    }

    #[test]
    fn a_later_record_supersedes_an_earlier_one() {
        let dir = scratch("supersede");
        let st = state(&dir);
        let base = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        st.record(base).unwrap();
        st.record(base + Duration::from_secs(300)).unwrap();
        assert_eq!(
            st.age(base + Duration::from_secs(310)).unwrap(),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn a_stamp_in_the_future_reads_as_unknown() {
        // The clock moved backwards, so the record is no longer a trustworthy
        // upper bound on the resting book's age — fall back to "unknown".
        let dir = scratch("future");
        let st = state(&dir);
        let then = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        st.record(then).unwrap();
        assert_eq!(st.age(then - Duration::from_secs(1)), None);
    }

    #[test]
    fn a_corrupt_record_reads_as_unknown() {
        let dir = scratch("corrupt");
        let st = state(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&st.path, b"{not json").unwrap();
        assert_eq!(st.age(SystemTime::now()), None);
    }

    #[test]
    fn two_markets_keep_separate_records() {
        // The per-market file is what keeps seven concurrent bot processes from
        // clobbering each other's timestamps.
        let dir = scratch("per-market");
        let store = QuoteStateStore::new(&dir);
        let a = store.for_market(Pubkey::new_unique(), "EURC");
        let b = store.for_market(Pubkey::new_unique(), "XSGD");
        let base = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        a.record(base).unwrap();
        b.record(base + Duration::from_secs(600)).unwrap();
        let now = base + Duration::from_secs(600);
        assert_eq!(a.age(now).unwrap(), Duration::from_secs(600));
        assert_eq!(b.age(now).unwrap(), Duration::ZERO);
    }

    #[test]
    fn no_temporary_file_survives_a_write() {
        let dir = scratch("no-temp");
        let st = state(&dir);
        st.record(SystemTime::now()).unwrap();
        let leftovers: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "stray temp files: {leftovers:?}");
    }
}
