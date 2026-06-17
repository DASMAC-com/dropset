---
name: firm-perms
description: Generalize the local permission allowlist into reusable globs — Bash commands and file-access/`Read` paths alike — harvesting everything you had to approve this session (or a pasted block of permission strings you want memorialized), and propagate it to the base-repo settings so future worktrees inherit it. Use at the end of a session, during a review-pr run that piled up per-worktree or per-arg approvals, or with a pasted permissions block to escape prompts you keep hitting.
user-invocable: true
---

# `firm-perms`

"Firm up" `.claude/settings.local.json`: rewrite
narrow, one-off `permissions.allow` entries into
generalized globs, dedupe them, and write the same
allowlist to **both** this worktree and the base
repo so the rules survive into future worktrees.

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

Optional, and accepts three shapes:

- **No argument** — firm the **entire** allow array
  (plus this session's harvested approvals, per the
  steps below).

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
   Bash commands, `Read(…)` file-access paths, and
   `additionalDirectories` grants (e.g. an "always allow
   access to …" you clicked through). For each, derive
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
