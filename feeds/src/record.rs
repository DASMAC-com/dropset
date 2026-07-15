//! The unit a source yields and the runner fans out.

use crate::cursor::Cursor;

/// A batch of typed records from a [`crate::Source`], plus the drive-loop
/// metadata the runner and store sink need.
pub struct Batch<R> {
    /// The records, oldest-first.
    pub records: Vec<R>,
    /// The source's resume position *after* these records. A poll source sets
    /// it so the store sink can persist a resumable cursor; a subscribe source
    /// leaves it `None` — a live stream has nothing to resume.
    pub cursor: Option<Cursor>,
    /// `true` once the source has reached the present: the runner sleeps
    /// `poll_interval` before the next call. `false` mid-backfill: the runner
    /// loops immediately to drain the backlog.
    pub caught_up: bool,
}

impl<R> Batch<R> {
    /// A caught-up batch with no cursor — the common shape for a subscribe
    /// source or a test. Poll sources add a cursor with [`Batch::with_cursor`]
    /// and mark backlog with [`Batch::caught_up`].
    pub fn new(records: Vec<R>) -> Self {
        Self {
            records,
            cursor: None,
            caught_up: true,
        }
    }

    /// Attach the resume cursor for this batch.
    pub fn with_cursor(mut self, cursor: Cursor) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// Set whether the source has caught up to the present.
    pub fn with_caught_up(mut self, caught_up: bool) -> Self {
        self.caught_up = caught_up;
        self
    }

    /// Whether this batch carries no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The number of records in this batch.
    pub fn len(&self) -> usize {
        self.records.len()
    }
}
