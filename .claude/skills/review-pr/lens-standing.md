# Standing scope, budget, and suppressions

The agent-facing half of every lens brief in a `review-pr` fan-out.
`lens_preamble.py --lens <name>` appends this to the canonical sub-agent
brief, then appends the named file from `lenses/`, and writes the result
to the scratchpad. The rationale for each rule lives in `history.md`
under step 5 or 6, beside the measurement that produced it.

## Scope of this review

Work **only** from the diff and commit log you were given. Dependency and
toolchain sources (`~/.cargo`, `node_modules`, another repo) are out of scope:
if you think you need a library's source to settle a point, say so in your
findings rather than going to look for it.

**Negative scope:** review the code diff only — do **not** audit permissions,
settings, or git history. Lenses have drifted into a permission-allowlist audit,
or run the full test suite instead of reviewing the diff, and the redo is
expensive. Stay on your dimension.

**Read the captured diff by path.** Your prompt names the diff file in the
scratchpad. Read that file; do not re-derive the diff by shelling out to
`git`.

## Budget

- Adjudicate from the provided diff plus the excerpts inlined in your prompt.
  **Cold-read only a file no excerpt covers.**
- Read every file you do need **once, up front**, and reason from that copy.
  Slice-read the large ones — Grep to the section, then `Read` with
  `offset`/`limit`. Re-reading a file on each turn pays for it on each turn.
- Your lens section below states a cap in tool calls, and usually in turns
  too. It is a
  **hard stop**: at the cap, report what you have and flag anything
  unresolved. Do not continue past it.
- **Do NOT re-open a file a finding already cites** unless you are resolving a
  specific, named dispute about that exact file.
- Where your material carries a hoisted grep result set, **adjudicate from
  the diff + the provided grep — don't re-derive** it with a sweep of your
  own.
- The exemplar to match: **read once, adjudicate, report.** The cheapest
  lenses on record finished in two turns, one with zero cold reads, all
  crediting the facts and excerpts they were handed.
- **A resume is a report-now order.** If you are resumed after stopping,
  report now in **at most 2 more tool calls**; do not start a new line of
  inquiry.

## The lint gate owns the compile-time facts

**Unused-import / does-it-resolve / does-it-compile / unused-symbol checks
are owned by the lint gate** the review runs before this fan-out: for
Rust `cargo clippy --all-targets -- -D warnings` **plus** `rustdoc` (a
full-workspace `cargo doc` under `-D warnings`, settling intra-doc links in
the program crate and its generated SDK copy); `ruff check` for Python;
`biome` and `tsc` for TS/JS. Never cold-read a file to confirm a symbol
resolves, and refute any finding resting on such a claim. Your facts name
the hook covering this diff and say when one could not run (a fresh
worktree with no frontend deps, say) or when lint was skipped or is
failing; in any of those cases the gate proves nothing, and you read for
yourself.

## Before you emit a finding

**Pre-emit gate — quote the changed line, or drop it.** Every finding quotes
the exact `+`/`-` diff line it rests on — or, for a claim that unchanged
text has gone stale, the excerpt line it rests on. One that cannot point at
its evidence in the material you were given is **dropped, not emitted** —
an unsupported finding costs more to adjudicate than it can be worth.
There is **no** numeric confidence scale; severity is **blocking** /
**warning** / **nit**.

A claim that the diff **violates a convention** cites the convention's
definition site, or is dropped. Confirm a rule's presence or absence by
reading the **current** file, never from the diff's `-`/`+` lines.

State your confidence plainly, and prefer a short, specific finding to a
long speculative one. If your
dimension turns up nothing, say so: a clean verdict is a result, not a
failure to look.

## Standing suppressions

Do not report any of the following — each is a deliberate, settled convention
of this repo, and re-litigating one wastes a round trip:

- The absence of AI attribution in commits or the PR body.
- Formatting that the lint hooks own (`rustfmt`, `ruff-format`, `biome`,
  `mdformat`, `taplo`). Lint runs separately and has already passed; a
  formatting nit here is noise. Flag a **lint-visible** problem only if you
  believe the hook itself is misconfigured, and say so in those terms.
- Missing `Co-Authored-By`, changelog entries, or version bumps — this repo
  uses none of them.
- `ENG-###` tags absent from the PR body: that is required, not an oversight.
- **Pre-existing code the diff doesn't touch** — judge the `+`/`-` lines and
  their immediate context.
- **Tuning constants / threshold values or their "why this value"
  comments.**
- **An assertion that "could be tighter"** when the existing one covers the
  behavior under test.
- **Harmless redundancy that aids readability** (an explicit match arm, a
  clarifying local binding).
- **Consistency-only nits** ("wrap X the way Y is guarded") when both forms
  are already correct.

These are **not** a free pass: a real bug on a suppressed *line* is still a
finding.

## Report

Return each finding with its file path, line number, **the exact `+`/`-`
diff line (or excerpt line) it rests on**, severity (**blocking** /
**warning** / **nit**), and a one-line rationale. Then:

- **Checks to run**: concerns you could not settle in scope, each phrased
  as the check that would settle it. Use this slot rather than speculating
  or staying silent.
- **Files opened**: every file you read, so an overrun is visible next
  time.
