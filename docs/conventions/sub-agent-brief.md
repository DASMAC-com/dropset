# Briefing sub-agents

A sub-agent you spawn (via the `Agent` tool) does **not** inherit the
project instructions, so left to itself it reaches for `find / …`,
`sed -n '…p' … | grep`, `cat`, and other compounds that can't reduce
to an allow-rule and re-prompt on **every** run — the exact churn the
[shell rules](shell-commands.md) exist to avoid. So whenever a skill
spawns a sub-agent, it must carry the conventions into the agent
itself. This is the single canonical brief; skills reference it by name
("prepend the sub-agent brief") rather than each pasting their own
copy, so the wording stays in one place.

**Prepend this standing brief to *every* `Agent` prompt:**

> - You are a **read-only** agent. The material you need to reason
>   over — a diff, a commit log, a set of issues — is included in this
>   prompt; start there, and you often won't need a shell at all.
> - To inspect files, prefer the **Read / Grep / Glob** tools over
>   `cat` / `head` / `tail` / `sed` / `awk` / `find` / `grep` in Bash —
>   they don't prompt for in-workspace paths, and they search other
>   directories too.
> - **Searching file *contents* — prefer the Grep tool; where it's
>   absent (native macOS builds), a bare single `grep` is the fallback,
>   but never a pipe and never `git grep`.** When Grep is present it
>   holds in-workspace *and* cross-path: Grep reads
>   any directory you point it at, takes a real regex (so an
>   alternation is `a|b|c`, not a shell-quoted `a\|b\|c`), and prompts
>   **zero** times. Do **not** reach for `git -C <path> grep …` to
>   search contents — `grep` is a git subcommand, so it looks blessed
>   by the cross-checkout rule below, but it isn't: a clean single
>   pattern only re-prompts until firmed, and a quoted `\|` alternation
>   trips the harness's per-subcommand `|` guard and **can't be firmed
>   at all**. Grep sidesteps both.
> - **Reading large files — Grep to the relevant section, then `Read`
>   with `offset`/`limit`.** Don't pull a whole `CLAUDE.md`, doc, or
>   SKILL.md into context to use a fraction of it; a whole-file Read of
>   a large file is a top token sink (see
>   [context economy](context-economy.md)).
> - **Read each file you need once, then reason from it.** Open the
>   handful of files your task touches a single time up front (slicing
>   the large ones as above) and work from what you've read — don't
>   re-`Read` or re-grep the same file on later turns. Every re-read is
>   paid again in your own context.
> - **Exploring another repo or path is fine** — reach outside this
>   worktree when the task needs it; approving a one-off read of a
>   different repo is expected, not something to avoid. Just keep each
>   access **globbable** so it approves once and won't re-prompt: use
>   Read / Grep / Glob for files and their contents, or read another
>   checkout's **metadata** with `git -C <path> <subcommand>` — `log` /
>   `show` / `diff` / `status` / `ls-files`, *not* `grep` (the subcommand
>   immediately after the path, no `cd`). What to avoid is the
>   **un-globbable** shape — a `find / …` sweep, or several `git -C …`
>   calls strung together with `&&` / `|` / `;` into one compound that
>   can't reduce to a rule.
> - **One bare command per Bash call** — no pipes, `&&`, `;`, command
>   substitution `$(…)`, redirects, or heredocs. Each call must reduce
>   to a `prefix:*` allow-rule.

**Pass the material inline.** Whatever the agent must reason over —
the diff, the commit log, the issue set — goes **in the prompt**, so
no agent re-fetches it by shelling out. (For content too large or
special-character-laden to sit inline cleanly, use the file-handoff
pattern from the shell rules — write it out and pass the path.)

**A skill may narrow this scope, never loosen it.** The brief is the
floor: shell discipline plus the freedom to explore. A spawning skill
is free to add a tighter *subject* scope on top — a diff reviewer, for
instance, should be told to "review only from the diff and commit log
below; dependency and toolchain sources are out of scope — flag it in
your findings instead of scanning." That narrows *where the agent
looks*; the shell rules stay exactly as written. An audit agent, by
contrast, is *meant* to range over the whole codebase, so it gets the
brief without any narrowing.

**State the negative scope, not just the positive one.** An agent
told only what to review will still wander off-lens — a code reviewer
drifts into a settings / permissions audit, a style pass runs the
whole test suite. So when a skill narrows the subject, give the agent
an explicit *negative* bound alongside the positive one — e.g.
"review the code diff only; do not audit permissions, settings, or git
history." One line naming what's **out** of scope is what keeps an
on-topic agent from straying into an expensive tangent.

A sub-agent approval that still re-prompts despite this brief means
the brief **leaked** — the agent emitted shell the brief forbids.
That's a prompt to tighten, not a rule to allow-list; `firm-perms`
sets such approvals aside and names the emitting agent so its prompt
gets fixed at the source.
