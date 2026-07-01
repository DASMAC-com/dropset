---
name: firm-perms
description: Generalize the local permission allowlist into reusable globs — Bash commands and file-access/`Read` paths alike. The low-friction default is the fast path: right after you one-time-approve a prompt, a bare `/firm-perms` (or `/firm-perms this`) firms just that one command immediately — no sweep, no confirm gate. Otherwise it runs the full sweep — harvest everything you approved this session (or a pasted block of permission strings) and propagate it. A firmed rule lands by classification: worktree-agnostic read-only rules go into the committed `.claude/settings.json` (so a fresh worktree inherits them through git, with no re-firming); everything else stays in the per-worktree `.claude/settings.local.json`. Use the fast path after an approval, the full sweep at the end of a session or during a review-pr run that piled up per-worktree or per-arg approvals, or with a pasted permissions block. `review-pr` drives it `base-only` at its merge-queue handoff (the worktree is about to be torn down).
user-invocable: true
---

<!-- cspell:word firmable -->

# `firm-perms`

"Firm up" the permission allowlist: rewrite narrow, one-off
`permissions.allow` entries into generalized globs, dedupe them, and
write each firmed rule to **the file its scope belongs in** —
worktree-agnostic read-only rules into the **committed**
`.claude/settings.json`, everything else into the per-worktree
`.claude/settings.local.json` (this worktree and the base repo). See
**Where firmed rules land** for the split and why it matters.

The common case is the **fast path**: right after you
one-time-approve a prompt, a bare `/firm-perms` (or
`/firm-perms this`) firms just that one command immediately
— no session sweep, no propose-then-confirm gate.
Everything below is
the **full sweep**, the heavier cleanup the fast path is
a shortcut around.

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
approvals too and writes the firmed result to the right
file so future worktrees inherit what they should.

Some approvals **can't** be firmed into a safe rule —
a heredoc, a `cd … &&` compound, a `python3`/`jq`
one-liner, anything CLAUDE.md's shell rules prohibit.
Those re-prompt because they're malformed, not
because a glob is missing; never allow-list them.
Surface them in the summary and point at the
offending pattern so the *author* (a skill, a script,
or you) stops emitting it.

## Where firmed rules land

There are **two** allowlist files, and the difference between them is
the whole reason this skill used to be run again in every fresh
worktree:

- **Committed `.claude/settings.json`** is tracked in git, so it rides
  into every `git worktree` checkout automatically. A rule here is
  inherited by every future worktree with **no re-firming** — this is
  the file that actually fixes the "cold worktree re-prompts until
  firm-perms runs again" churn.
- **Per-worktree `.claude/settings.local.json`** is gitignored and
  exists as an **independent copy per worktree**, not a symlink. The
  harness merges a session's settings only from its **own** project
  root (plus the user/managed scopes) — never from an ancestor
  directory — and an untracked file never rides into a fresh checkout.
  So a rule written *only* to `settings.local.json` is **not** inherited
  by future worktrees; it lives and dies with the worktree it was
  written in (and the base copy, which seeds nothing on its own because
  the seed mechanism — a tracked file — is `settings.json`).

So each firmed rule is **classified** by scope and written to the file
that matches:

- **Worktree-agnostic *and* read-only → committed `.claude/settings.json`.**
  The read-only git verbs are the canonical set: `git status:*`,
  `git diff:*`, `git log:*`, `git show:*`, `git rev-parse:*`,
  `git branch:*`, `git ls-files:*`, `git ls-tree:*`, `git ls-remote:*`,
  `git merge-base:*`, `git reflog:*`, `git check-ignore:*`,
  `git worktree list:*`, and the
  `git -C <base>/.claude/worktrees/* <read-verb>:*`
  globs (the tag already collapsed to `*`, so the rule is the same in
  every worktree). The companion read-only `gh` reads
  (`gh pr checks:*`, `gh pr view:*`, `gh api graphql:*`) and the
  read-only GitHub MCP tools belong here too, on the same rationale
  (routine, low-blast-radius, identical across worktrees — see
  `docs/conventions/github-mcp.md`). A rule is committed-eligible only
  when it is **both** worktree-agnostic and clearly read-only — with
  **one enumerated exception**: the narrow, routine PR-lifecycle
  **writes** that every worktree runs and that `docs/conventions/github-mcp.md`
  explicitly blesses for inheritance (the GitHub MCP PR-authoring writes
  and `init-pr`'s notification-subscription ignore) are committed too,
  because they fire in every fresh worktree and re-prompting defeats the
  purpose. That's an explicit list in `github-mcp.md`, **not** a class
  this skill auto-promotes — firm-perms never moves an arbitrary write
  into the committed file on its own.
- **Everything else → per-worktree `.claude/settings.local.json`**, in
  **this (the active) worktree** (so the rule takes effect now) **and**
  the **base repo** (so a hand-run firm in one worktree at least
  carries to the base copy). This is the right home for mutations
  (`git commit`, `git push`, `cargo build`, `pnpm install`, `rm`,
  `mv`), anything network- or mutation-capable, machine-specific
  absolute `Read` paths, and `additionalDirectories` grants — rules
  that are either not safe to share by default or not identical across
  machines/worktrees.

It deliberately does **not** fan out to sibling
worktrees for the local file. An existing sibling keeps its own
`settings.local.json` until it's either recreated (re-seeded) or
re-firmed while it's itself the active worktree; the committed
`settings.json` rules it already has from git. That narrower scope is
the accepted tradeoff: a sibling may re-prompt once for a *local* rule
firmed here until it re-firms itself, but the skill no longer reads and
rewrites every live worktree on every run.

Don't hardcode the base-repo path — discover it at
run time (step 1). The examples below use `<base>`
for that path and `<tag>` for a worktree folder
(e.g. `eng-447`).

### `base-only` (driven by `review-pr`)

`review-pr` calls this skill at its merge-queue handoff, when the
active worktree is **about to be torn down** — a write to that
worktree's `settings.local.json` is wasted churn. So `review-pr` passes
**`base-only`** (e.g. `/firm-perms base-only`, optionally with a
fragment). In `base-only`:

- **Skip the active-worktree `settings.local.json` write entirely.**
  Local-classified rules are written only to the **base** copy.
- Committed `.claude/settings.json` rules are still written normally —
  that file *is* the base (it's tracked, one copy), and it's what
  future worktrees inherit.
- Report that only the base was firmed (committed `settings.json` and
  the base `settings.local.json`), not the active worktree.

Any other invocation (interactive `/firm-perms`, fast path, full sweep)
writes the active worktree as usual.

## Input

Optional, and accepts these shapes:

- **Bare `/firm-perms` right after a one-time
  approval**, or **`/firm-perms this`** — the **fast
  path**: firm just the command you literally just
  approved, into the file its scope belongs in,
  immediately, no sweep, no confirm gate. This is the
  primary low-friction entry point; see the **Fast path**
  under Steps.

- **No argument** (when the previous turn was *not* a
  one-time approval) — the full sweep: firm the
  **entire** allow array (plus this session's harvested
  approvals, per the steps below).

- **A fragment** (e.g. a rule that just got added, or a
  command you keep approving) — focus the generalization
  on the matching entries but still dedupe and write the
  whole array.

- **`base-only`** (optionally with a fragment) — the
  full sweep, but with the active-worktree
  `settings.local.json` write suppressed; see
  **`base-only`** above. `review-pr` passes this at its
  merge-queue handoff.

- **A pasted permissions block** — one or more raw
  command / rule strings, typically the ones a
  permission prompt just surfaced, often under a
  `# Permissions` heading or in a fenced block. This is
  the "memorialize these so I stop being asked"
  entry-point. Treat
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

  Then run the normal generalize / dedupe / classify /
  propose / write flow over the lines that can be firmed
  and the whole array.

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
   verbatim. (These are still **classified** by the
   rule in "Where firmed rules land" — a read-only MCP
   tool can go committed; a machine-specific absolute
   `Read` path and `additionalDirectories` stay local.)

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
  pasted permissions block, or `base-only`. It harvests
  the whole session (or the given source) and runs the
  propose-then-confirm flow. See **Full sweep** below.

`base-only` is a **modifier** on the full sweep (it only
changes which files get the local-classified rules — see
"Where firmed rules land"), not a third mode.

### Fast path (firm the just-approved command)

The low-friction common case: the user one-time-approves
a prompt (option 1, because option 2's "don't ask again
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
   stop and ask the user how to narrow it. This is
   the one case the fast path is allowed to pause.
1. **Classify the rule** per "Where firmed rules land":
   worktree-agnostic *and* read-only → the **committed**
   `.claude/settings.json`; anything else → the
   per-worktree `.claude/settings.local.json` in this
   worktree **and** the base.
1. **Find the base repo** exactly as the full sweep's
   step 1 does (`git worktree list --porcelain`). The
   `refs/heads/main` worktree is `<base>`.
1. **Read the file(s) the classification targets** with
   the Read tool — the committed `.claude/settings.json`
   (this worktree's, which is the same tracked file
   everywhere) for a committed-eligible rule, and/or this
   worktree's and the base's `.claude/settings.local.json`
   for a local rule. Don't read a file you won't write.
1. **Write the glob into the classified file's `allow`
   array immediately**, with Edit/Write — **no
   propose-then-confirm gate** (the deliberate difference
   from the full sweep). Dedupe against the file's
   existing entries; if the glob is already present or
   subsumed by a broader existing rule there, no-op for
   that file. Leave `additionalDirectories` and every
   other key intact. For a local-classified rule, write
   both `settings.local.json` copies (this worktree and
   the base) so they match; for a committed rule, the one
   tracked `settings.json` is the write. If a base /
   committed write is denied (the path isn't in this
   session's `additionalDirectories`), say so and report
   what was actually firmed rather than implying more —
   same caveat as the full sweep.
1. **Report in one line** what was added, to which file,
   and that the copies now match — e.g. "Firmed
   `Bash(git diff:*)` into the committed `settings.json`"
   or "Firmed `Bash(cargo test -p dropset:*)` into this
   worktree + base `settings.local.json`." Because the
   report states exactly what was written and where, the
   change stays trivially reversible.

**Why no confirm gate here.** The full sweep's
propose-then-confirm gate exists because a sweep can
touch many rules at once and resurrect drifted entries.
The fast path touches exactly one
rule that the user *just* approved by hand and explicitly
asked to firm, and it reports precisely what it wrote —
so the human confirmation already happened (the
one-time approval plus the `/firm-perms`), and the
safety floor still blocks the one dangerous outcome (a
bare-verb wildcard). The full-sweep modes keep their
gate unchanged.

### Full sweep

The remaining modes — no-arg full harvest, fragment,
pasted block, and `base-only` — run the harvest-and-propose
flow below.

1. **Find the base repo.** List worktrees and read
   the path out of the output yourself (no command
   substitution):

   ```sh
   git worktree list --porcelain
   ```

   The worktree whose `branch` line is
   `refs/heads/main` is the base repo (`<base>`). Take
   its literal path from the output. The committed
   `.claude/settings.json` is one tracked file; the
   local `.claude/settings.local.json` lands in this (the
   active) worktree and `<base>` — except under
   `base-only`, where the active-worktree local write is
   skipped (see "Where firmed rules land"). Sibling
   worktrees are not touched. If no worktree has `main`
   checked out, warn the user and firm only this worktree
   (local) — there's no base to write.

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

1. **Read the allowlists** with the Read tool (per the
   CLAUDE.md shell conventions — never shell out to
   `jq`/`node`/`python` to read or edit JSON):

   - the committed `.claude/settings.json` (one tracked
     file)
   - this worktree's `.claude/settings.local.json`
   - `<base>/.claude/settings.local.json`

   (Under `base-only` you may skip the active worktree's
   local file — it won't be written.)

1. **Build the firmed allowlist, then classify.** Union
   the `allow` arrays you just read with the
   session-harvested rules from the harvest step, apply
   the generalization rules above, and dedupe. Then
   **classify** each resulting rule per "Where firmed
   rules land" — worktree-agnostic read-only into the
   committed-`settings.json` set, everything else into
   the local-`settings.local.json` set. Three cautions:

   - **Watch entries that live in only one file.** The
     copies can have drifted, and a rule missing from one
     may have been *deliberately* dropped there.
     Don't silently resurrect it everywhere; treat every
     entry present in only one file as a distinct diff
     item the user has to approve in the next step.
   - **Run the safety floor over the *result*, not just
     over rules you generalized.** A pre-existing
     bare-verb wildcard (e.g. a stray `git *` that crept
     into one copy) would otherwise ride the union
     straight into a file untouched. Flag any
     such entry instead of propagating it.
   - **A rule that moves files counts as a diff item.**
     If a read-only git rule currently sits in
     `settings.local.json` and the classification now
     routes it to the committed `settings.json`, show
     that move in the proposal (it's the mechanism that
     drains the cold-worktree churn) so the user sees the
     committed file growing.

1. **Propose, then wait for the user.** Before
   writing **anything**, show the user exactly what
   will change and why, and stop for their go-ahead.
   Present it as a concrete diff against the current
   allowlists, **labeled by destination file**:

   - each rule being **added** (the generalized glob),
     **which file** it lands in (committed `settings.json`
     vs. `settings.local.json`), and the concrete rule(s)
     it replaces, with a one-line reason (e.g. "collapses
     three per-worktree variants into one"; "read-only
     git — promoted to committed `settings.json`");
   - each rule being **removed** as a now-subsumed
     duplicate, or **moved** from the local file to the
     committed one;
   - each entry present in **only one** of the files,
     so the user can confirm it should land where you
     propose (rather than having drifted out on purpose);
   - any over-broad rule you're **flagging** but
     leaving in place (per the safety floor).

   Do not edit any file until the user approves.
   If they want changes, adjust the proposal and show
   it again. This confirmation gate matters most for
   the committed `settings.json` and the base-repo local
   file, since those are what seed every future worktree.

1. **Write it to the classified files** once approved,
   with Edit/Write — replacing only the `allow` array in
   each and leaving `additionalDirectories` (and any
   other keys) intact:

   - **Committed `.claude/settings.json`** — the
     worktree-agnostic read-only set. One tracked file;
     written the same in `base-only` as in any other mode
     (it *is* shared).
   - **`.claude/settings.local.json`** — the local set,
     to this worktree **and** `<base>` so both match —
     **except under `base-only`**, where only the base
     copy is written (the active worktree is being torn
     down).

   Writing a base / committed copy reaches outside this
   worktree, so it only works when `<base>` is in this
   session's `additionalDirectories` (it normally is). If
   that write is denied, say so and report what was
   actually firmed rather than implying the base /
   committed file was updated — don't leave the user
   thinking future worktrees were covered when they
   weren't.

1. **Report.** Confirm what was written and to **which
   file** — the committed `settings.json` rules (the ones
   future worktrees now inherit through git) and the
   local `settings.local.json` copies (this worktree and
   the base, or base-only) — or, if a write was denied,
   what was actually firmed. List
   the session approvals you firmed in (Bash and
   file-access alike), and separately the **malformed**
   approvals you set aside — name the offending pattern
   (heredoc, `cd … &&`, `python3`/`jq` one-liner) and
   point at its source so the author stops emitting it,
   rather than allow-listing it.
