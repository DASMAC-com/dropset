---
name: firm-perms
description: Generalize the local permission allowlist into reusable globs — Bash commands and file-access/`Read` paths alike. The low-friction default is the fast path: right after you one-time-approve a prompt, a bare `/firm-perms` (or `/firm-perms this`) firms just that one command into both the worktree and base-repo settings immediately — no sweep, no confirm gate. Otherwise it runs the full sweep — harvest everything you approved this session (or a pasted block of permission strings, or the Linear "Permissions" inbox doc) and propagate it to the base-repo settings so future worktrees inherit it. Use the fast path after an approval, the full sweep at the end of a session or during a review-pr run that piled up per-worktree or per-arg approvals, with a pasted permissions block, or pass `doc` to drain the Linear Permissions inbox.
user-invocable: true
---

<!-- cspell:word firmable -->

# `firm-perms`

"Firm up" `.claude/settings.local.json`: rewrite
narrow, one-off `permissions.allow` entries into
generalized globs, dedupe them, and write the same
allowlist to **both** this worktree and the base
repo so the rules survive into future worktrees.

The common case is the **fast path**: right after you
one-time-approve a prompt, a bare `/firm-perms` (or
`/firm-perms this`) firms just that one command into
both files immediately — no session sweep, no
propose-then-confirm gate. Everything below is the
**full sweep**, the heavier cleanup the fast path is a
shortcut around.

This is the cleanup pass for the allowlist churn
that builds up as you approve commands by hand. It
both **harvests** every approval you had to grant
this session — Bash commands *and* file-access
(`Read`) paths and `additionalDirectories`, not just
the rules already written to disk — and
**generalizes and propagates** them. It overlaps the
built-in `fewer-permission-prompts` skill (which
discovers new *read-only Bash* rules from your
transcripts) but is broader: it folds in file-access
approvals too and writes the firmed result to the
base repo so future worktrees inherit it.

Some approvals **can't** be firmed into a safe rule —
a heredoc, a `cd … &&` compound, a `python3`/`jq`
one-liner, anything CLAUDE.md's shell rules prohibit.
Those re-prompt because they're malformed, not
because a glob is missing; never allow-list them.
Surface them in the summary and point at the
offending pattern so the *author* (a skill, a script,
or you) stops emitting it.

## Why both files

`.claude/settings.local.json` is gitignored and
exists as an **independent copy per worktree**, not
a symlink. A running session reads its *own*
worktree copy, and new worktrees are seeded from the
**base repo** file. So a rule has to land in both
places: the worktree copy to stop prompting *now*,
and the base copy to reach *future* worktrees. This
skill always writes the identical, firmed allowlist
to both.

Don't hardcode the base-repo path — discover it at
run time (step 1). The examples below use
`<base>` for that path and `<tag>` for a worktree
folder (e.g. `eng-447`).

## Input

Optional, and accepts these shapes:

- **Bare `/firm-perms` right after a one-time
  approval**, or **`/firm-perms this`** — the **fast
  path**: firm just the command you literally just
  approved into both settings files immediately, no
  sweep, no confirm gate. This is the primary
  low-friction entry point; see the **Fast path** under
  Steps.

- **No argument** (when the previous turn was *not* a
  one-time approval) — the full sweep: firm the
  **entire** allow array (plus this session's harvested
  approvals, per the steps below).

- **A fragment** (e.g. a rule that just got added, or a
  command you keep approving) — focus the generalization
  on the matching entries but still dedupe and write the
  whole array.

- **A pasted permissions block** — one or more raw
  command / rule strings, typically the ones a
  permission prompt just surfaced, often under a
  `# Permissions` heading or in a fenced block. This is
  the "memorialize these so I stop being asked"
  entry-point (you can drop such a block straight into a
  Linear task and run `/firm-perms` against it). Treat
  **each line** as a session approval to fold in: derive
  the generalized rule it *should* have been (per the
  generalization rules below) and add it to the working
  set, exactly as the harvest step does for approvals
  you granted live. A line that **can't** reduce to a
  safe rule — a heredoc, a `cd … &&` compound, a
  `python3` / `jq` one-liner, anything CLAUDE.md's shell
  rules forbid — is malformed, not missing a glob: a `*`
  can't rescue it and allow-listing wouldn't even stop
  the prompt. So for those, **fix the source instead of
  the allowlist** — which is the other half of "stop
  being asked":

  - If the pattern traces to a **committed skill,
    script, Makefile target, or doc** (its shell is
    baked in, so it re-emits every run), edit that
    source so it stops producing the malformed command —
    e.g. replace a `python3 … | yaml` parse with the
    Read tool, split a `cd … &&` compound into bare
    commands. Propose that edit alongside the allowlist
    diff (next steps) and apply it on approval.
  - If it was a **one-off** you (or a sub-agent) typed
    ad-hoc — no committed source emits it — there's
    nothing to edit; name it in the summary, cite the
    CLAUDE.md rule it broke, and move on.

  Then run the normal generalize / dedupe / propose /
  write flow over the lines that can be firmed and the
  whole array.

- **The Linear "Permissions" doc** (pass `doc`, or run
  by `housekeeping`) — drain Alex's living
  **Permissions** inbox document, where he dumps
  permission prompts as they fire across sessions.
  Each unchecked `- [ ]` entry holds a captured prompt
  block; adjudicate the command it contains, firm it or
  file a source-fix task, and record the disposition
  back into the doc. This mode reuses the same
  generalization and adjudication rules but sources its
  commands from the doc instead of the session and
  writes its results back into it — see **Draining the
  Linear Permissions doc** below.

## Generalization rules

Apply these conservatively — the goal is to widen
the *variable* parts (worktree tag, trailing args,
throwaway paths) while keeping the **command +
subcommand prefix literal** so a rule never grants
more verb than it used to.

1. **Collapse worktree tags to `*`.** Any literal
   `.claude/worktrees/<tag>` path segment becomes
   `.claude/worktrees/*`. E.g.
   `git -C <base>/.claude/worktrees/<tag> status --short`
   → `git -C <base>/.claude/worktrees/* status:*`.
   This applies to file-access rules too:
   `Read(//<base>/.claude/worktrees/<tag>/**)` →
   `Read(//<base>/.claude/worktrees/**)`.

1. **Generalize trailing args with the `:*`
   suffix.** A rule pinned to concrete args loses
   them in favor of `:*` (the canonical "any args"
   form Claude Code itself writes — equivalent to a
   space followed by `*`, whose word boundary keeps
   `status:*` matching `status` as a whole word and
   not as a prefix of some longer command). So
   `… status --short` → `… status:*`,
   `git add -A` → `git add:*`,
   `tee /tmp/eng447_nt2.txt` → `tee /tmp/*`.

1. **Keep the subcommand.** Generalize args, never
   the verb. `git -C … status --short` firms to
   `git -C … status:*`, **not** `git -C … :*` or
   `git *`. The literal `git -C <path> <subcommand>`
   prefix stays.

1. **Dedupe.** After generalizing, collapse exact
   duplicates and any narrow rule now subsumed by a
   broader one (e.g. drop `… worktrees/eng-447 status:*`
   once `… worktrees/* status:*` exists). Preserve
   first-occurrence order otherwise.

1. **File-access rules get the same treatment.**
   `Read(…)` path rules and `additionalDirectories`
   entries are firmed exactly like Bash paths —
   collapse any `worktrees/<tag>` segment to
   `worktrees/*` (rule 1) and dedupe — so a path
   approved in one worktree covers them all.
   `WebFetch(domain:…)`, `mcp__…`, and `Skill(…)` rules
   carry no per-worktree path, so copy those through
   verbatim.

### Safety floor — do not over-widen

The matcher treats a `*` as "any characters,
including spaces," so an over-broad rule is a real
hazard. Never produce a bare-verb wildcard like
`git *`, `gh *`, `pnpm *`, `cargo *`, or `rm *` —
keep at least the command **and** its subcommand
literal. If the allowlist already contains such an
over-broad rule, **do not silently delete it**;
surface it in your summary and recommend the user
tighten it, then leave it as-is unless they say
otherwise.

(Compound commands re-prompt regardless: Claude Code
validates each `&&` / `|` / `;` / `&` subcommand
against the allowlist independently, so a `*` in a
rule can't smuggle a second command past it. That's
a reason to keep authoring one bare command per Bash
call, not a reason to widen rules.)

## Steps

**Classify the invocation first.** This skill has two
entry points, and a bare `/firm-perms` picks between
them by context:

- **Fast path** — fires when either `/firm-perms this`
  is typed (the word "this", or a fragment matching the
  just-approved command), **or** a bare `/firm-perms` is
  invoked and the immediately-preceding turn was a
  one-time permission approval. It firms exactly that one
  just-approved command, in place, with no sweep and no
  confirm gate. See **Fast path** immediately below.
- **Full sweep** — every other invocation: a bare
  `/firm-perms` when the previous turn was *not* an
  approval, a fragment given for general cleanup, a
  pasted permissions block, or `doc`. It harvests the
  whole session (or the given source) and runs the
  propose-then-confirm flow. See **Full sweep** below.

### Fast path (firm the just-approved command)

The low-friction common case: Alex one-time-approves a
prompt (option 1, because option 2's "don't ask again
for…" is almost always far too broad — `pnpm *`,
`git *`), then types `/firm-perms` to memorialize the
*correct* narrow glob right now. This path does only
that — it does **not** harvest the rest of the session,
and it does **not** propose-then-wait.

1. **Identify the target command** — the single Bash
   command (or `Read` / file-access path, or `WebFetch`
   domain) from the immediately-preceding approved tool
   call. That one command is the entire scope; the rest
   of the session is not touched.
1. **Generalize it** with the **Generalization rules**
   above, unchanged — collapse `worktrees/<tag>` to `*`,
   suffix trailing args with `:*`, keep the command +
   subcommand literal, dedupe.
1. **Apply the Safety floor** above, unchanged. If the
   only safe generalization the rules can produce would
   be a bare-verb wildcard (`git *`, `pnpm *`,
   `cargo *`, `gh *`, `rm *`), **do not write it** —
   stop and ask Alex how he wants to narrow it. This is
   the one case the fast path is allowed to pause.
1. **Find the base repo** exactly as the full sweep does
   (step 1 below: `git worktree list --porcelain`; the
   `refs/heads/main` worktree is `<base>`).
1. **Read both allowlists** with the Read tool — this
   worktree's `.claude/settings.local.json` and
   `<base>/.claude/settings.local.json`.
1. **Write the glob into both `allow` arrays
   immediately**, with Edit/Write — **no
   propose-then-confirm gate** (the deliberate difference
   from the other modes). Dedupe against the existing
   entries; if the glob is already present or subsumed by
   a broader existing rule, no-op and say so. Leave
   `additionalDirectories` and every other key intact;
   both files end byte-identical. If the base-repo write
   is denied (base not in `additionalDirectories`), say
   so and report that only the worktree copy was firmed
   — same caveat as the full sweep.
1. **Report in one line** what was added and that both
   copies now match — e.g. "Firmed
   `Bash(cargo test -p dropset:*)` into worktree + base."
   Because the report states exactly what was written,
   the change stays trivially reversible.

**Why no confirm gate here.** The full sweep's
propose-then-confirm gate exists because a sweep can
touch many rules at once and resurrect drifted entries
into the base file. The fast path touches exactly one
rule that Alex *just* approved by hand and explicitly
asked to firm, and it reports precisely what it wrote —
so the human confirmation already happened (the
one-time approval plus the `/firm-perms`), and the
safety floor still blocks the one dangerous outcome (a
bare-verb wildcard). The full-sweep modes keep their
gate unchanged.

### Full sweep

The remaining modes — no-arg full harvest, fragment,
pasted block, and `doc` — run the harvest-and-propose
flow below.

1. **Find the base repo.** List worktrees and read
   the path out of the output yourself (no command
   substitution):

   ```sh
   git worktree list --porcelain
   ```

   The worktree whose `branch` line is
   `refs/heads/main` is the base repo (`<base>`).
   Take its literal path from the output. If no
   worktree has `main` checked out, warn the user and
   firm only this worktree's file.

1. **Harvest this session's approvals.** Beyond what's
   already on disk, scan the current session for every
   permission you had to approve by hand — not just
   piled-up Bash globs. `fewer-permission-prompts` does
   this for read-only Bash; here include **all** of it:
   Bash commands, `Read(…)` file-access paths,
   `additionalDirectories` grants (e.g. an "always allow
   access to …" or "allow reading from `alex/`" you
   clicked through), and URL approvals
   (`WebFetch(domain:…)`). For each, derive
   the generalized rule it *should* have been (per the
   rules above) and add it to the working set.

   **Sub-agent approvals count too.** A command a
   spawned sub-agent ran (e.g. `review-pr`'s diff-review
   and cross-check agents) still surfaced to *you* for
   approval, so it's part of this session's churn —
   harvest it exactly like a command you typed. When
   such an approval is **malformed** and gets set aside
   (below), name the agent that emitted it in the
   summary, so its prompt or brief can be tightened at
   the source.

   Exception: an approval that re-prompts because it's
   **malformed** — a heredoc, a `cd … &&` compound, a
   `python3` / `jq` one-liner, anything CLAUDE.md's
   shell rules forbid — does **not** become a rule. A
   `*` can't rescue a compound (Claude Code re-validates
   each sub-command), so allow-listing it wouldn't even
   stop the prompt. Set these aside for the summary
   instead (see the intro).

1. **Read both allowlists** with the Read tool (per
   the CLAUDE.md shell conventions — never shell out
   to `jq`/`node`/`python` to read or edit JSON):

   - this worktree's `.claude/settings.local.json`
   - `<base>/.claude/settings.local.json`

1. **Build the firmed allowlist.** Union both `allow`
   arrays with the session-harvested rules from the
   harvest step, apply the generalization rules above,
   and dedupe — this is the single canonical array both
   files will get. Two cautions on the union:

   - **Watch entries that live in only one file.** The
     two copies can have drifted, and a rule missing
     from the base may have been *deliberately* dropped
     there. Don't silently resurrect it into both;
     treat every one-file-only entry as a distinct diff
     item the user has to approve in the next step.
   - **Run the safety floor over the *result*, not just
     over rules you generalized.** A pre-existing
     bare-verb wildcard (e.g. a stray `git *` that crept
     into one copy) would otherwise ride the union
     straight into the base file untouched. Flag any
     such entry instead of propagating it.

1. **Propose, then wait for the user.** Before
   writing **anything**, show the user exactly what
   will change and why, and stop for their go-ahead.
   Present it as a concrete diff against the current
   allowlist:

   - each rule being **added** (the generalized glob)
     and the concrete rule(s) it replaces, with a
     one-line reason (e.g. "collapses three
     per-worktree variants into one");
   - each rule being **removed** as a now-subsumed
     duplicate;
   - each entry present in **only one** of the two
     files, so the user can confirm it should land in
     both (rather than having drifted out on purpose);
   - any over-broad rule you're **flagging** but
     leaving in place (per the safety floor).

   Do not edit either file until the user approves.
   If they want changes, adjust the proposal and show
   it again. This confirmation gate matters most for
   the base-repo (meta-level) file, since it seeds
   every future worktree.

1. **Write it to both files** once approved, with
   Edit/Write — replacing only the `allow` array and
   leaving `additionalDirectories` (and any other
   keys) intact. Both files end byte-identical.
   Writing the base copy reaches outside this
   worktree, so it only works when the base repo is in
   this session's `additionalDirectories` (it normally
   is). If that write is denied, say so and report that
   only the worktree copy was firmed — don't leave the
   user thinking future worktrees were covered when
   they weren't.

1. **Report.** Confirm what was written and that both
   the worktree and base-repo copies now match. List
   the session approvals you firmed in (Bash and
   file-access alike), and separately the **malformed**
   approvals you set aside — name the offending pattern
   (heredoc, `cd … &&`, `python3`/`jq` one-liner) and
   point at its source so the author stops emitting it,
   rather than allow-listing it.

## Draining the Linear Permissions doc

Alex keeps a living **Permissions** document in Linear
— an inbox where he dumps permission prompts as they
fire across multi-session work, under headings like
`# /review-pr agents` and `# General/unknown`. Each
prompt is captured as an unchecked `- [ ]` item whose
body is a fenced block holding the command (and often
the surrounding "Do you want to proceed?" chrome and
the over-broad `2. Yes, and don't ask again for: git *`
option the harness offered). This mode drains that
inbox: it adjudicates each unchecked item with the
**same** generalization rules and safety floor as
every other mode, then records the disposition back
into the doc.

It runs in **two contexts**, which differ only in
**autonomy** (what they're allowed to write), never in
how they adjudicate:

- **Attended** — you invoke `/firm-perms doc` yourself.
  This may write `settings.local.json` (behind the
  normal propose-then-confirm gate), tick the doc
  checkboxes, and finalize the notes.
- **Propose-only** — `housekeeping` invokes it
  unattended on a timer. It **never writes settings**
  and **never ticks a checkbox**. It only annotates
  each unchecked item with the disposition it
  *recommends*, and files source-fix tasks for
  malformed entries. The actual settings write and
  check-off are left for a later attended run. (This is
  the deliberate autonomy bound: an unattended loop must
  not silently widen the base-repo allowlist that seeds
  every future worktree.)

The mode is selected by the caller: `housekeeping`
passes `doc propose-only`; a bare `doc` is attended.

### Steps (doc mode)

1. **Resolve the doc id from the environment**, on the
   same bare-`printenv` rule as the other Linear ids
   (one variable per call — never a combined
   `printenv A B`):

   ```sh
   printenv LINEAR_PERMISSIONS_DOC_ID
   ```

   If it's empty, say so and stop — don't guess a doc
   id.

1. **Read the doc live, every pass.** Fetch it fresh
   with `mcp__claude_ai_Linear__get_document` (id = the
   resolved value). Never reuse a snapshot from an
   earlier pass: Alex adds entries quickly, so a stale
   body would drop his newest items or clobber his
   edits on write-back.

1. **Find the work.** Collect every **unchecked**
   (`- [ ]`) entry; ticked (`- [x]`) ones are already
   disposed, so skip them. Then triage each unchecked
   entry by the disposition note (if any) a prior pass
   left under it — this is what makes the loop converge
   instead of re-firming the same rules forever:

   - **No disposition note** → fresh work; adjudicate it
     below.
   - **`✓ firmed: <rule>`** (an attended firm that Alex
     re-opened) → a **contest**; handle it per **The
     contest protocol** below (attended reverts the
     rule; propose-only skips).
   - **`⚠ contested — reverted …`** → held for Alex;
     skip it until he edits the entry (see the contest
     protocol).
   - **`recommend firm: …`** or **`⚠ can't firm: …`** (a
     prior propose-only recommendation) → an attended
     run executes the recommendation (firm it / confirm
     the filed task); propose-only skips it as already
     handled.

   For each entry you do adjudicate, extract the actual
   command from its fenced block: ignore the prompt
   chrome — the `Bash command` header, the description
   line, the `Do you want to proceed?` prompt, and its
   numbered menu — and the
   `2. Yes, and don't ask again for: …` line — that
   line shows the **over-broad** rule the harness
   offered (often a bare-verb `git *`), which the safety
   floor forbids; derive the correct narrow glob
   yourself, don't copy it. Some entries are truncated
   or garbled (e.g. a line starting `rs/alex/…`); if you
   can't recover a runnable command, treat it as
   malformed-unrecoverable and note that rather than
   guessing.

1. **Adjudicate each command** with the existing rules
   — no new logic:

   - **Reduces to a safe glob** → it's firmable. Derive
     the generalized rule per the **Generalization
     rules** above (collapse `worktrees/<tag>` to `*`,
     `:*` the trailing args, keep the command +
     subcommand literal, respect the safety floor — no
     bare-verb wildcards).
   - **Malformed** — a compound (`&&`, `;`), a pipe, a
     `$(…)` substitution, a redirect, a heredoc, or an
     `awk`/`sed`/`python3`/`jq` one-liner that should be
     a Read/Grep/Glob tool call or a lint hook — **can't
     be allow-listed** (a `*` can't rescue it, and the
     harness re-validates each sub-command). It must be
     fixed at the **source**. If a committed skill,
     script, Makefile target, or sub-agent emits it,
     **file a Linear task** to fix that source so the
     prompt stops firing; if it was a one-off nobody
     emits, there's nothing to fix — just record that.
   - **Network- or mutation-capable verb** (e.g.
     `gh api …`, which can `-X DELETE`) — even when it
     reduces to a glob, do **not** auto-firm it in
     propose-only mode. Flag it for the attended run to
     decide, with a note on why it needs a human.

1. **Apply the disposition — gated by autonomy:**

   - **Attended (`doc`)**: fold the firmable rules into
     the working set and run them through the normal
     **propose → confirm → write-both-files** flow
     (steps 5–6 above) — the base-repo write still waits
     for your go-ahead. For malformed entries, file the
     source-fix task (below). Then **write the doc back**
     (next step): tick each disposed item and replace its
     note.
   - **Propose-only (`doc propose-only`, housekeeping)**:
     do **not** touch `settings.local.json` and do
     **not** tick any checkbox. For firmable entries,
     annotate the item with the rule you *would* firm
     and why. For malformed entries, file the source-fix
     task (filing a task is proposing a fix, not widening
     settings — it's allowed unattended). Leave the
     checkbox unchecked so the attended run still sees it
     as work, but skip any item that already carries a
     disposition note — one whose nested line begins with
     `recommend firm:`, `✓ firmed:`, `⚠ can't firm:`, or
     `⚠ contested —` — so a 30-minute loop doesn't
     re-annotate or re-file what it (or an attended run)
     already handled. Match the actual lead markers the
     write-back step emits, not a paraphrase, so the skip
     reliably fires.

1. **Filing a source-fix task** (malformed entries).
   Use the same env-resolved destination as
   `linear-task` / `housekeeping` (resolve each id with
   its own bare `printenv` — `LINEAR_TEAM_ID`,
   `LINEAR_PROJECT_ID`, `LINEAR_ASSIGNEE_ID`),
   `state: "Backlog"`, priority 3, with a fingerprint
   line so re-runs dedup. Before filing, list the open
   Backlog and skip any issue already carrying the same
   fingerprint:

   ```txt
   mcp__claude_ai_Linear__save_issue(
     team: "<$LINEAR_TEAM_ID>",
     project: "<$LINEAR_PROJECT_ID>",
     assignee: "<$LINEAR_ASSIGNEE_ID>",
     state: "Backlog",
     title: "<source>: stop emitting <malformed pattern>",
     description: "<the captured command + which CLAUDE.md
       rule it breaks + the fix>\n\n**Fingerprint**:
       perms-doc:<short-hash-or-slug>",
     priority: 3,
   )
   ```

1. **Write the disposition back into the doc.** There
   is no per-line comment API on a Linear *document*, so
   the record lives **inline in the body**: rewrite the
   doc with `mcp__claude_ai_Linear__save_document` (id =
   the resolved value), and for each disposed entry add
   nested note lines indented under it. Keep the
   captured block intact; just append the note and (when
   attended) flip the checkbox:

   ```md
   - [x] git -C …/worktrees/eng-531 ls-files cfg
     - ✓ firmed: `Bash(git -C …/worktrees/* ls-files:*)`
     - reason: read-only `ls-files` on a sibling
       worktree; tag collapsed to `*`, args to `:*`
   ```

   For a malformed entry:

   ```md
   - [x] ls /Users/alex/repos 2>/dev/null | grep -i drop
     - ⚠ can't firm: pipe + redirect compound — filed
       ENG-### to fix the stage-backlog cross-check
       agent's brief (use Glob, not `ls | grep`)
   ```

   In **propose-only** mode the same notes are written
   but the checkbox stays `- [ ]` and the lead marker is
   `recommend firm:` rather than `✓ firmed:` (with the
   recommended rule, e.g. `Bash(git -C …/worktrees/* diff:*)`).

   **Diff against the live body before saving.** Build
   the new body from the body you just fetched this pass,
   changing only the lines you're disposing; never
   reorder or rewrite Alex's other content. If the doc
   `updatedAt` is newer than when you fetched it
   (someone edited mid-pass), re-fetch and rebuild rather
   than overwriting his change.

### The contest protocol

An auto-firm can be wrong, so Alex needs a way to
reverse one. The protocol uses the checkbox he can see:

- To **contest** a firm, Alex **re-opens** the item —
  flips `- [x]` back to `- [ ]` — leaving its
  `✓ firmed: <rule>` note in place.
- On the next **attended** pass, an item that is
  **unchecked but still carries a `✓ firmed: <rule>`
  note** is the contest signal. **Revert** that exact
  rule from **both** `settings.local.json` copies
  (worktree + base), then replace the note with a
  `⚠ contested — reverted <rule>; needs re-handling`
  note and leave the item unchecked.
- That `⚠ contested — reverted` note then **holds the
  item for Alex** — it is *not* fresh work. Don't
  re-adjudicate or re-firm a contested item on a later
  pass; that would just re-fire the rule Alex rejected
  and loop. The item only re-enters adjudication once
  Alex acts on it: he rewrites the command, deletes the
  note, or removes the entry. Until then, skip it (the
  step-3 "find the work" pass excludes it).

So an unchecked item resolves unambiguously by its
note: a `✓ firmed:` note means *contest → revert*; a
`⚠ contested — reverted` note means *held, skip*; no
disposition note means *fresh work → adjudicate*.

Propose-only (housekeeping) never reverts settings and
never re-handles a contested item; it just leaves any
already-noted entry for the attended run, exactly as
its skip rule (step "apply the disposition") already
provides.
