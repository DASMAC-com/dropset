<!-- cspell:word backticked -->

# `plan` close-out

The close-out branch of [`SKILL.md`](SKILL.md) step 6, and its
step 7 tail. Read this file when the close-out is offered or
`cycle` runs it; the in-session write rules stay in the skill.

## Gate the close-out; don't wait to be told

When the operator signals wrap-up, or the conversation lulls after
significant decisions, offer via `AskUserQuestion` — "close out
now? (brings the Planning doc to the current state of play)" —
recommended first.

**On the yes, stop the curator first.** `ListAgents`; a live
curator gets one `SendMessage`, "closing for the day", and its
`curate` stop tail ends in `CLEAR TO CLOSE`. Do not wait for an
answer. None live: report `curator: not live`.

## The deliverable: the whole document, section-patched

**The close-out deliverable is the whole document, brought** to
the ephemeral shape the skill's "The Planning document is the
living state" section names and nothing else: superseded
phases, dated addenda and the session's incremental notes
disappear (it is still no append log: `In-session notes` is
swept), and no ruling, doctrine or method text survives outside
`In-session notes`. **Restore `Discuss with operator` to a
single blank checkbox**, once its items are acted on, and carry
no operator-actions list: an open operator action is an
`[Operator]` issue, never document text. **Write only the
sections that changed**, never the whole body: put each changed
top-level section in one file and run
`planning_doc.py replace-sections --file <path>` (`--drop '# <heading>'`
deletes one; `--dry-run` first). A section owns its
subsections: re-supply each one changed, `--keep` one carried
unchanged (always the machine-written upkeep report, so the live
copy is written, never a stale paste), or `--drop` it, else the
write is refused.
Unchanged sections are carried byte for byte, any heading not
matching exactly once refuses the whole write, and the tool
reads the document back itself — exit 1 names the first
differing line, so slice it. A section written this way still
re-parses on the round trip, so compose it:

- **without inline code spans** — plain names, not backticked;
- with **no emphasis span crossing a newline**;
- with **no machine-parsed field starting with a bare
  hostname-valid `name.ext`**.

These rules live in the `plan` skill only (this file); the
document's one-line header points at the skill rather than
restating them.

## The audit-state table and the upkeep report

**The close-out carries the audit-state table forward — and
writes no directive** (why: [`history.md`](history.md)). The
table is state-shaped: a row is **replaced** when its unit is
audited again, so its size is bounded by the registry, and the
board holds the history. A table growing rows per audit has
become the log it exists to avoid. **Update the rows for
whatever landed this cycle** — an audit issue pulled and
completed sets its unit's date, count and pointer; one filed but
unpulled changes nothing, since the row records what was
audited, not scheduled. **Carry the upkeep report forward
verbatim** — its heading through its `End of upkeep report.`
line: the tool splices only between the two and refuses a
section whose end line is gone.

## Dismiss the settled PRs' notifications

One bare call, so the day's merges leave no inbox behind them:

```sh
python3 .claude/tools/pr_notifications.py dismiss --settled --apply
```

It selects every thread whose PR is merged or closed, whatever
its issue's state, never an open PR's, marks each done with one
call (the merge tail already unsubscribed each merged one), and
prints counts, not threads; say its `summary` line in the
close-out report. A failure is that line, never a blocker.

## Step 7: `/session-metrics`

Run `/session-metrics`, the shared tail, before the final
document write. Its id comes from
`resolve_session.py --daily-id plan` (`--date YYYYMMDD`,
`--cycle N` for an earlier one; [`history.md`](history.md)), and
the rewrite carries its `session_metrics.py --cost-line` output
as one line under `Session-metrics state`.
