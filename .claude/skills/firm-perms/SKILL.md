---
name: firm-perms
description: Generalize the local permission allowlist into reusable globs and propagate it to the base-repo settings so future worktrees inherit it. Use after you've had to approve per-worktree or per-arg `git -C …/worktrees/<tag> …` (or similar) variants and want to collapse them so they stop re-prompting.
disable-model-invocation: true
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
is the sibling of the built-in `fewer-permission-prompts`
skill: that one *discovers new* read-only rules to
add from your transcripts; this one *generalizes
and propagates* the rules you already have.

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

Optional. With no argument, firm the **entire**
allow array. If given a fragment (e.g. a rule that
just got added, or a command you keep approving),
focus the generalization on the matching entries but
still dedupe and write the whole array.

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

1. **Leave the rest untouched.** `WebFetch(domain:…)`,
   `mcp__…`, `Skill(…)`, `Read(…)`, and the
   `additionalDirectories` array are copied through
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

1. **Read both allowlists** with the Read tool (per
   the CLAUDE.md shell conventions — never shell out
   to `jq`/`node`/`python` to read or edit JSON):

   - this worktree's `.claude/settings.local.json`
   - `<base>/.claude/settings.local.json`

1. **Build the firmed allowlist.** Take the union of
   both `allow` arrays, apply the generalization
   rules above, and dedupe. This is the single
   canonical array both files will get.

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

1. **Report.** Confirm what was written and that both
   the worktree and base-repo copies now match.
