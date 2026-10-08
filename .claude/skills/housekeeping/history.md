# `housekeeping` history ledger

The measured incidents behind the rules in [`SKILL.md`](SKILL.md),
kept here so the entry file states each rule once. Read on demand,
never on invocation.

## Step 8: auto-memory review

- **Index lines capped at write time.** Per-entry hook lines accreted
  detail over many sessions, several running 200–400 characters, and
  the cost landed all at once: one session's single largest result
  (≈5.2k) was a whole-file Read of the index, forced by a size hook
  demanding compaction, plus a full rewrite of 71 entries.
- **The scan became a tool.** The step was the last one in the pass
  still prescribed as prose, so every pass improvised the same
  shapes: ≈3.2k of ≈8.3k total Bash bytes (~39%) on one measured
  pass, four of its top six results — including an `ls` that printed
  all 97 filenames to answer what the already-in-context index
  answers, and an `awk` that returned 56 rows when the decision
  needed a count.
- **Whole-index byte cap.** The index was 19,404 bytes when the cap
  was proposed, resident every turn of every session including
  Bedrock workers; the per-line width bound alone did not stop it
  growing one line per memory.
