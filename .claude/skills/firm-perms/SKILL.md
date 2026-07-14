---
name: firm-perms
description: Generalize the local permission allowlist into reusable globs — Bash commands and file-access/`Read` paths alike. Bare `/firm-perms` (or `/firm-perms this`) is the deterministic fast firm: it memorializes the single command you just approved into this worktree's and the base repo's settings immediately, via the same `firm_last.py` tool `/f` runs — no sweep, no confirm gate. `/firm-perms sweep` (or a pasted permissions block) runs the full harvest-and-propose sweep: collect everything approved this session, generalize and dedupe it, and propagate the result to this worktree and the base repo behind a propose-then-confirm gate. Use the fast firm after an approval, the sweep at the end of a session or to reconcile worktree/base drift.
user-invocable: true
---

# `firm-perms`

"Firm up" `.claude/settings.local.json`: rewrite narrow, one-off
`permissions.allow` entries into generalized globs, dedupe them, and
write the same allowlist to **this worktree and the base repo** — so the
rule takes effect here *now* and the base copy seeds it into future
worktrees.

This skill has **two modes**, chosen by the invocation, with **no
inference** between them:

- **Fast firm** — bare `/firm-perms`, or `/firm-perms this`: firm the
  single command you just approved. This is a thin alias for the `/f`
  skill's tool; it delegates to `firm_last.py` and does nothing else.
- **Full sweep** — `/firm-perms sweep`, or a pasted permissions block:
  the heavier harvest-and-propose cleanup.

Earlier versions tried to *guess* which mode a bare `/firm-perms` meant
by classifying whether the previous turn was an approval. The transcript
gives no clean signal for that, so it misfired into the full sweep. The
guess is gone: bare `/firm-perms` is **always** the fast firm.

## Fast firm (bare `/firm-perms` / `/firm-perms this`)

The low-friction common case: you one-time-approve a prompt (option 1 —
option 2's "don't ask again for…" is almost always far too broad:
`pnpm *`, `git *`), then firm the *correct* narrow glob right now. This
does only that — it does **not** harvest the rest of the session, and it
does **not** propose-then-wait.

It is identical to `/f`: run the deterministic tool, which finds the most
recent executed tool call, generalizes it, and writes the rule into this
worktree's and the base repo's `settings.local.json`:

```sh
python3 .claude/tools/firm_last.py
```

Relay the tool's one-line result. `firm-perms this` behaves the same
(the word "this" just names the just-approved command explicitly). If
the verbatim command is what you want rather than the generalized glob,
that maps to `/f exact` — `python3 .claude/tools/firm_last.py exact`. See
the [`/f` skill](../f/SKILL.md); the two are the same tool.

## Generalization and coverage rules

Both modes generalize and dedupe by the **same rules**, and those rules
live in **one place**: the Python module `.claude/tools/firm_core.py`
(`generalize`, `is_covered`, `is_bareverb_wildcard`), which the fast-firm
tool calls directly. To check whether a **single** candidate rule is
already granted without whole-reading a `settings.local.json` into
context (per `CLAUDE.md` → "Context economy"), ask the allowlist helper —
it prints `{covered, insertion_index, would_subsume, count}`:

```sh
python3 .claude/tools/allowlist.py \
  --settings <path>/.claude/settings.local.json covers 'Bash(git status:*)'
```

When you generalize by hand in the full sweep, follow what `firm_core`
does — don't re-derive a different behavior:

1. **Collapse worktree tags to `*`.** Any literal
   `.claude/worktrees/<tag>` path segment becomes
   `.claude/worktrees/*` — in Bash rules and `Read(…)` paths alike.
1. **Generalize trailing args with `:*`.** A rule pinned to concrete
   args loses them in favor of `:*` (the canonical "any args" form,
   equivalent to a trailing space-`*`). So `… status --short` →
   `… status:*`, `git add -A` → `git add:*`.
1. **Keep the command + subcommand literal.** Generalize args, never
   the verb: `git -C <path> status --short` firms to
   `git -C <path> status:*`, never `git -C … :*` or `git *`. Stable
   value-flags that name a path/dir (`-C <path>`, `--dir frontend`) stay
   in the literal prefix with their value.
1. **Dedupe.** Collapse exact duplicates and any narrow rule now
   subsumed by a broader one (`is_covered` decides this). Preserve
   first-occurrence order otherwise.
1. **`WebFetch(domain:…)`, `mcp__…`, and `Skill(…)` carry no
   per-worktree path** — copy them through verbatim.

### Safety floor — do not over-widen

The matcher treats `*` as "any characters, including spaces," so an
over-broad rule is a real hazard. Never produce a bare-verb wildcard
like `git *`, `gh *`, `pnpm *`, `cargo *`, or `rm *` — keep at least the
command **and** its subcommand literal. `firm_core.is_bareverb_wildcard`
encodes this floor, and the fast-firm tool **refuses** to write such a
rule (it asks you to narrow by hand). In the full sweep, apply the same
floor over the *result*: if the allowlist already contains such an
over-broad rule, do **not** silently delete it — surface it in your
summary and recommend the user tighten it, then leave it as-is unless
they say otherwise.

(Compound commands re-prompt regardless: Claude Code validates each
`&&` / `|` / `;` / `&` subcommand against the allowlist independently,
so a `*` in a rule can't smuggle a second command past it. That's a
reason to keep authoring one bare command per Bash call, not to widen
rules.)

Some approvals **can't** be firmed into a safe rule — a heredoc, a
`cd … &&` compound, a `python3` / `jq` one-liner, anything the shell
conventions prohibit. Those re-prompt because they're malformed, not
because a glob is missing (`firm_core.generalize` returns `None` for
them); never allow-list them. Surface them and point at the offending
pattern so the *author* (a skill, a script, or you) stops emitting it.

## Why base + active worktree

`.claude/settings.local.json` is gitignored and exists as an
**independent copy per worktree**, not a symlink. A running session
reads its *own* worktree copy, and new worktrees are seeded from the
**base repo** file. So this skill writes the firmed allowlist to exactly
two places: **this (the active) worktree** — so the rule takes effect in
the running session *now* — and the **base repo** — so every *future*
worktree inherits it on creation.

It deliberately does **not** fan out to sibling worktrees. An existing
sibling keeps its own allowlist until it's either recreated (re-seeded
from the base) or re-firmed while it's itself the active worktree. That
narrower scope is the accepted tradeoff.

Don't hardcode the base-repo path — discover it at run time (sweep step
1). The examples below use `<base>` for that path.

## Full sweep (`/firm-perms sweep` / pasted block)

The full sweep is the heavier cleanup for allowlist churn that has piled
up over a session, or to reconcile drift between a worktree and the
base. It runs only when explicitly asked:

- **`/firm-perms sweep`** — harvest the **entire** session (plus the
  whole allow array) and run the propose-then-confirm flow below.

- **A pasted permissions block** — one or more raw command / rule
  strings (often under a `# Permissions` heading or in a fenced block),
  the "memorialize these so I stop being asked" entry-point. Treat
  **each line** as a session approval to fold in: derive the generalized
  rule it *should* have been (per the rules above) and add it to the
  working set. A line that **can't** reduce to a safe rule is malformed,
  not missing a glob — fix the source instead of the allowlist:

  - If the pattern traces to a **committed skill, script, Makefile
    target, or doc**, edit that source so it stops producing the
    malformed command (e.g. replace a `python3 … | yaml` parse with the
    Read tool, split a `cd … &&` compound). Propose that edit alongside
    the allowlist diff and apply it on approval.
  - If it was a **one-off** you or a sub-agent typed ad-hoc, there's
    nothing to edit; name it in the summary, cite the rule it broke, and
    move on.

### Sweep steps

1. **Find the base repo.** List worktrees and read the path out of the
   output yourself (no command substitution):

   ```sh
   git worktree list --porcelain
   ```

   The worktree whose `branch` line is `refs/heads/main` is the base
   repo (`<base>`). Take its literal path from the output — the firmed
   allowlist lands in just two places, this (the active) worktree and
   `<base>`; sibling worktrees are not touched. If no worktree has
   `main` checked out, warn the user and firm only this worktree.

1. **Harvest this session's approvals.** Beyond what's already on disk,
   scan the session for every permission you had to approve by hand —
   Bash commands, `Read(…)` file-access paths, `additionalDirectories`
   grants, and URL approvals (`WebFetch(domain:…)`). For each, derive
   the generalized rule it *should* have been and add it to the working
   set. **Sub-agent approvals count too** — a command a spawned
   sub-agent ran still surfaced to you for approval; harvest it like a
   command you typed, and when a malformed one is set aside, name the
   agent that emitted it so its brief can be tightened.

   Exception: an approval that re-prompts because it's **malformed**
   (a heredoc, a `cd … &&` compound, a `python3` / `jq` one-liner) does
   **not** become a rule — a `*` can't rescue a compound. Set these
   aside for the summary.

   **Gate the dual read on this harvest.** If the working set is
   **empty** after harvesting — no permission prompt fired this session
   (and no pasted block was supplied) — there is nothing to firm: report
   "nothing to firm" and **stop here, without reading either
   allowlist**. Both `settings.local.json` copies are large; reading
   them to union-and-diff is pure overhead when the mature allowlist
   already covered every command this session (the common case). Only
   when the harvest yields **≥1** new / uncovered rule is the dual read
   below worth its cost.

   The empty harvest is **terminal — do not read the allowlist to
   "double-check" it.** The harvest is authoritative: it already tells
   you whether any prompt fired, and reading `settings.local.json` to
   re-confirm the working set is empty defeats the whole point of the
   gate (that read has been the top single token sink of a firm-nothing
   sweep). An empty harvest means *stop and report*, full stop — no
   confirming read of either copy. In practice most sessions firm
   nothing, so this gate fires far more often than the dual read does.

1. **Read both allowlists** with the Read tool (never shell out to
   `jq` / `node` / `python` to read or edit JSON):

   - this worktree's `.claude/settings.local.json`
   - `<base>/.claude/settings.local.json`

1. **Build the firmed allowlist.** Union the two `allow` arrays with the
   session-harvested rules, apply the generalization rules above
   (`firm_core` is the reference), and dedupe — one canonical array both
   files will get. Two cautions on the union:

   - **Watch entries that live in only one file.** The two copies can
     have drifted, and a rule missing from the base may have been
     *deliberately* dropped there. Treat every entry present in only one
     file as a distinct diff item the user has to approve.
   - **Run the safety floor over the *result*.** A pre-existing bare-verb
     wildcard that crept into one copy would otherwise ride the union
     into the base file untouched. Flag it instead of propagating it.

1. **Propose, then wait for the user.** Before writing **anything**,
   show a concrete diff against the current allowlist and stop for
   go-ahead:

   - each rule being **added** (the generalized glob) and the concrete
     rule(s) it replaces, with a one-line reason;
   - each rule being **removed** as a now-subsumed duplicate;
   - each entry present in **only one** of the files, so the user can
     confirm it should land in both;
   - any over-broad rule you're **flagging** but leaving in place.

   Do not edit any file until the user approves. This confirmation gate
   matters most for the base-repo file, since it seeds every future
   worktree.

1. **Write it to both files** once approved, with Edit/Write — replacing
   only the `allow` array and leaving `additionalDirectories` (and any
   other keys) intact. Writing the base copy reaches outside this
   worktree, so it only works when `<base>` is in this session's
   `additionalDirectories`. If the base write is denied, say so and
   report that only this worktree was firmed.

1. **Report.** Confirm what was written and that both copies now match
   (or that only this worktree was firmed). List the session approvals
   you firmed in, and separately the **malformed** approvals you set
   aside — name the offending pattern and point at its source so the
   author stops emitting it, rather than allow-listing it.
