---
name: f
description: Fast-firm the permission you just approved. Type /f right after one-time-approving a prompt to memorialize that single command as a reusable allow-glob in this worktree's and the base repo's settings — no sweep, no confirm gate. /f exact firms the command verbatim instead of generalized. This is the shorthand around firm-perms' fast path.
disable-model-invocation: true
user-invocable: true
---

# `/f`

Firm the command you **just approved** into the allowlist, right now.

`/f` is typed immediately after you one-time-approve a permission
prompt, so the just-approved command is deterministically the **most
recent executed tool call** in the session transcript. This skill firms
exactly that one — no session sweep, no propose-then-confirm gate.

It is a thin wrapper: the whole job is the deterministic tool
`.claude/tools/firm_last.py` (which resolves the transcript, finds the
most recent executed call, generalizes it via `firm_core.py`, and writes
the rule into this worktree's *and* the base repo's
`settings.local.json`). Run it, passing `$ARGUMENTS` straight through:

```sh
python3 .claude/tools/firm_last.py $ARGUMENTS
```

- Bare **`/f`** → generalize the command (collapse the worktree tag,
  keep the command + subcommand literal, `:*` the trailing args) and
  firm that glob.
- **`/f exact`** → firm the command **verbatim** (worktree tag still
  collapsed), when the generalized glob would be wrong or too broad.

Then relay the tool's one-line result verbatim — it states exactly what
rule was written and where (this worktree + base), so the change stays
trivially reversible.

The tool handles the edge cases itself, so just report what it prints:

- If the last call **can't reduce to a safe rule** (a compound, heredoc,
  or interpreter one-liner), it says so and firms nothing — the fix is
  to stop the *source* emitting that shape, not to allow-list it.
- If generalizing would produce an **over-broad bare-verb wildcard**
  (`git:*`, `pnpm:*`, `rm:*`), it refuses and asks you to narrow it by
  hand. Try `/f exact` if the verbatim command is what you want.
- If the rule is **already covered**, it no-ops and says so.

Never firm a "don't ask again"-style broad approval this way — `/f` is
for the narrow one-time approval you just granted. (The tool only ever
firms a single command and never widens to a bare verb, so this holds by
construction.)

For the heavier cleanup — harvesting a whole session's approvals,
reconciling worktree/base drift, or memorializing a pasted permissions
block — use `/firm-perms sweep` instead.
