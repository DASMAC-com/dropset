//! Paged backfill for poll sources: emit a backlog oldest-first, one page
//! at a time, so the resume cursor only ever advances over a contiguous
//! prefix (docs/data-feeds.md §13).
//!
//! A poll transport that answers "what is new since X" almost always
//! answers it **newest-first and capped** — `getSignaturesForAddress`
//! returns at most `limit` signatures, newest first, and a REST candle
//! endpoint behaves the same way. A source that takes such a page and
//! advances its cursor to the newest entry **skips the middle** of any
//! backlog deeper than one page: the records between that page's oldest
//! entry and the previous cursor are never fetched, and the cursor is now
//! past them, so they are never fetched later either.
//!
//! **The walk must finish before anything is emitted.** The tempting
//! bounded fix — walk back a few pages, emit the oldest one reached,
//! advance, repeat — is wrong, and wrong in the same direction as the bug it
//! is meant to fix. The resume cursor is an *exclusive lower bound*
//! (`until`): moving it to a mid-backlog position tells the next request to
//! ignore everything older, so any page below the one just emitted is
//! discarded rather than deferred. The oldest unemitted record is therefore
//! only identifiable once the walk has reached the resume bound. This pager
//! enforces that: it hands back pages to emit only after the walk reports it
//! got there, and until then it asks for more enumeration.
//!
//! **A page key addresses a page; it is not the page.** The pager stores one
//! key per page rather than the records, which is what keeps a deep
//! backlog's bookkeeping small enough that the walk can afford to run to the
//! bound. Re-requesting the page from its key at emission time costs one
//! cheap call against the many per-record fetches emitting it already
//! implies. A key has to address the page *exactly* — an RPC source keys on
//! the `before` marker **and** the newest signature the page held when it
//! was enumerated, because a window bounded only from below can grow at the
//! tip between enumeration and emission.
//!
//! **Emission is two-phase.** [`Backfill::step`] only looks; the page stays
//! queued until [`Backfill::commit`]. A source that fails partway through —
//! and the runner keeps a source alive across errors rather than rebuilding
//! it from the durable cursor — must retry the same page, not skip to the
//! next one and advance the cursor over the gap.

// cspell:word unemitted

/// What a source should do next, from [`Backfill::step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackfillStep<K> {
    /// The walk has not reached the resume bound yet: enumerate further
    /// before emitting anything.
    Enumerate,
    /// Emit this page — the oldest not-yet-emitted one. Advancing the resume
    /// cursor to its newest record is safe, because everything older has
    /// already been emitted.
    ///
    /// **The page stays queued until [`Backfill::commit`].** `step` only
    /// looks; a source that fails partway through emitting — a network
    /// error mid-fetch, a malformed response — must be able to retry the
    /// same page, and the runner keeps the source alive across an error
    /// rather than rebuilding it from the durable cursor. Popping here
    /// instead would strand that page: the retry would take the *next*
    /// one and the cursor would advance past the gap.
    Emit(K),
    /// Every discovered page has been emitted and the walk reached the
    /// bound: the source is current.
    Done,
}

/// The keys of discovered-but-unemitted pages, drained oldest-first.
///
/// A source discovers pages by walking **backwards** from the present (each
/// page older than the last), reporting each segment of the walk with
/// [`Backfill::extend`], and asks [`Backfill::step`] what to do. The pager
/// holds no cursor of its own — only the source knows how to read a position
/// out of a page — and is transport-agnostic in `K`, the source's own page
/// key.
pub struct Backfill<K> {
    /// Page keys in walk order: newest-discovered first, so the oldest page
    /// sits at the end and emission pops from the back.
    pending: Vec<K>,
    /// Whether the walk has reached the resume bound. Until it has, no page
    /// may be emitted — see the module note.
    reached_bound: bool,
}

impl<K> Default for Backfill<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K> Backfill<K> {
    /// A pager with no walk yet: the first [`Backfill::step`] asks the source
    /// to enumerate.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            reached_bound: false,
        }
    }

    /// Record one segment of the backward walk.
    ///
    /// `keys` are in walk order — **newest page first** — and are appended,
    /// so a walk split across several polls composes into one ordering.
    /// `reached_bound` says whether the walk has now reached the resume
    /// cursor (or the start of history); while it is `false` the pager keeps
    /// asking for more enumeration.
    pub fn extend(&mut self, keys: impl IntoIterator<Item = K>, reached_bound: bool) {
        self.pending.extend(keys);
        self.reached_bound = reached_bound;
    }

    /// What to do next: enumerate further, emit the oldest unemitted page, or
    /// stop because the source is current.
    ///
    /// This only **looks** — the page it names stays queued until
    /// [`Backfill::commit`], so a source that fails partway through emitting
    /// retries the same page rather than skipping it.
    pub fn step(&self) -> BackfillStep<K>
    where
        K: Clone,
    {
        if !self.reached_bound {
            return BackfillStep::Enumerate;
        }
        match self.pending.last() {
            Some(key) => BackfillStep::Emit(key.clone()),
            None => BackfillStep::Done,
        }
    }

    /// Discard the page [`Backfill::step`] just named, having emitted it.
    ///
    /// Call this **only once the batch is built** — after it, that page is
    /// unrecoverable, and the source is asserting the resume cursor has
    /// advanced over it.
    pub fn commit(&mut self) {
        self.pending.pop();
    }

    /// Whether the source has reached the present: the walk got to the bound
    /// **and** everything it found has been emitted. This is the `caught_up`
    /// a poll source reports, so the runner keeps looping while a backlog
    /// drains and only sleeps once the feed is genuinely current.
    pub fn caught_up(&self) -> bool {
        self.reached_bound && self.pending.is_empty()
    }

    /// Discard all walk state, so the next [`Backfill::step`] starts a fresh
    /// walk from the present. A source calls this once a backlog is drained.
    pub fn restart(&mut self) {
        self.pending.clear();
        self.reached_bound = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing may be emitted until the walk reports it reached the bound —
    /// the pager keeps asking for enumeration however many pages are queued.
    #[test]
    fn withholds_every_page_until_the_walk_reaches_the_bound() {
        let mut pager = Backfill::new();
        assert_eq!(pager.step(), BackfillStep::Enumerate);

        // A capped walk segment: two pages found, bound not yet reached.
        pager.extend(vec!["newest", "middle"], false);
        assert_eq!(pager.step(), BackfillStep::Enumerate);
        assert!(!pager.caught_up());

        // The next segment reaches it, and only now does emission start —
        // from the oldest page of the whole walk, not of the last segment.
        pager.extend(vec!["oldest"], true);
        assert_eq!(pager.step(), BackfillStep::Emit("oldest"));
        pager.commit();
        assert_eq!(pager.step(), BackfillStep::Emit("middle"));
        pager.commit();
        assert_eq!(pager.step(), BackfillStep::Emit("newest"));
        pager.commit();
        assert_eq!(pager.step(), BackfillStep::Done);
    }

    /// A walk that reached the bound in one segment drains oldest-first, and
    /// only reports caught-up once its last page is out.
    #[test]
    fn drains_a_complete_walk_oldest_first() {
        let mut pager = Backfill::new();
        pager.extend(vec!["newest", "middle", "oldest"], true);

        assert_eq!(pager.step(), BackfillStep::Emit("oldest"));
        pager.commit();
        assert!(!pager.caught_up());
        assert_eq!(pager.step(), BackfillStep::Emit("middle"));
        pager.commit();
        assert!(!pager.caught_up());
        assert_eq!(pager.step(), BackfillStep::Emit("newest"));
        pager.commit();
        assert!(pager.caught_up());
        assert_eq!(pager.step(), BackfillStep::Done);
    }

    /// `step` only looks: without a `commit` it keeps naming the same page,
    /// which is what lets a source retry an emission that failed partway
    /// through instead of stranding that page below an advancing cursor.
    #[test]
    fn step_is_repeatable_until_committed() {
        let mut pager = Backfill::new();
        pager.extend(vec!["newest", "oldest"], true);

        assert_eq!(pager.step(), BackfillStep::Emit("oldest"));
        // A failed emission commits nothing — the same page comes back.
        assert_eq!(pager.step(), BackfillStep::Emit("oldest"));
        assert_eq!(pager.step(), BackfillStep::Emit("oldest"));

        pager.commit();
        assert_eq!(pager.step(), BackfillStep::Emit("newest"));
    }

    /// A walk that found nothing leaves the source caught up and idle.
    #[test]
    fn an_empty_walk_is_caught_up() {
        let mut pager = Backfill::<&str>::new();
        pager.extend(Vec::new(), true);
        assert!(pager.caught_up());
        assert_eq!(pager.step(), BackfillStep::Done);
    }

    /// Restarting drops the finished walk so the next poll enumerates from
    /// the present again.
    #[test]
    fn restart_returns_the_pager_to_enumerating() {
        let mut pager = Backfill::new();
        pager.extend(vec!["only"], true);
        assert_eq!(pager.step(), BackfillStep::Emit("only"));
        pager.commit();
        assert!(pager.caught_up());

        pager.restart();
        assert!(!pager.caught_up());
        assert_eq!(pager.step(), BackfillStep::Enumerate);
    }

    /// Restarting mid-drain discards the queued pages too, so a source that
    /// restarts (rather than committing) cannot later emit a page addressed
    /// against a window it has since abandoned.
    #[test]
    fn restart_discards_pages_still_queued() {
        let mut pager = Backfill::new();
        pager.extend(vec!["newest", "oldest"], true);
        assert_eq!(pager.step(), BackfillStep::Emit("oldest"));

        pager.restart();
        assert_eq!(pager.step(), BackfillStep::Enumerate);

        // Nothing from the abandoned walk survives into the next one: a
        // fresh walk that finds nothing reports Done, not the stale page.
        pager.extend(Vec::new(), true);
        assert_eq!(pager.step(), BackfillStep::Done);
    }
}
