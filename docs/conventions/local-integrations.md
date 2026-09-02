<!-- cspell:word zshrc -->

<!-- cspell:word reorderer -->

<!-- cspell:word venv -->

<!-- cspell:word repoint -->

# Local integrations

This doc covers the **user-local Claude Code configuration** the repo
*documents but does not commit*: the compound-shell guard hook, the
git-grep guard hook, the worktree edit-path guard hook, the
destructive-command guard hook, the iTerm2
tab-color integration, and the shell setup they lean on. None of it is
enforced on a checkout.

**One exception, and it is new:** the **session helpers** are
*committed*, at `.claude/shell/init.zsh` — the shell profile keeps only
a one-line `source` of them, and only their 1Password coordinates stay
untracked. See "Session helpers" below for why that half moved into the
repo while everything else here stayed out of it.

Both `.claude/settings.json` (hook + permission wiring) and
`.claude/settings.local.json` (the per-machine allowlist) are
git-ignored — the repo *documents* how to configure your own Claude
Code, it does not push hooks or permissions onto a contributor or a CI
runner. So everything here is **opt-in**: a fresh worktree or
a new contributor gets none of it until they wire it into their own
local `settings.json`. That is the intended tradeoff — configuration is
the user's, not the checkout's.

The `$CLAUDE_PROJECT_DIR` variable used in the wiring below resolves to
the active checkout root, so the same `settings.json` block works in the
base repo and in every worktree.

**A committed guard is inert until it is wired.** Committing the script
is *not* the job — half of it is, and it is the half that leaves no
trace when the other half is missing. A guard with no `PreToolUse` entry
pointing at it never runs, while the repo goes on documenting it as a
protection; the script sitting there in `.claude/hooks/` reads as
evidence that it does. Two of the three guards below spent an unknown
stretch in exactly that state, discovered only when someone asked an
unrelated question about hook reach (2026-08-14).

Because the wiring is git-ignored, **CI cannot check this** — a PR
cannot install wiring and a CI runner has none to inspect. The check
therefore runs where the settings actually resolve, on the operator's
machine:

```sh
make hook-wiring
```

It names every committed hook nothing points at, and **writes
nothing** — wiring a hook grants it the right to block tool calls,
which stays the operator's decision. `housekeeping` runs it each pass
(its step 7b) so the gap cannot re-open silently.

`housekeeping` deliberately calls
`python3 .claude/tools/hook_wiring.py` rather than this `make` target:
that spelling already falls under the pre-approved
`Bash(python3 .claude/tools/:*)` rule, so an unattended pass never
stops to ask. The `make` target is the operator-facing spelling. Both
run the same check — don't collapse them into one.

## The compound-shell guard hook

The [shell-commands](shell-commands.md) conventions are enforced
**mechanically**, not just by convention. A `PreToolUse` Bash hook
(`.claude/hooks/no_compound_bash.py`) inspects each Bash command before
it runs and **blocks** any that contains an unquoted shell compound /
redirect operator — a pipe, `>`, `<`, `;`, `&&`, `||`, `&`, a backtick,
or `$(` — telling the model to split the call and use the Write / Read /
Grep tools instead. The scan is **quote-aware**: an operator inside a
single- or double-quoted string (a commit message's `;`, a regex's `|`)
is legitimate text and passes; command substitution (`` ` `` and `$(`)
is caught even inside double quotes, mirroring real shell. The guard
fails *open* — any payload it can't parse is allowed — so it never
wedges a session.

**Escape hatch.** A genuinely-unavoidable compound (rare) is let
through by adding the literal marker `#compound-ok` anywhere in the
command. It's deliberately visible in the transcript so the bypass is
auditable; reach for it only when the work truly can't be split.

### Wiring the compound-shell guard

The guard **script** (`.claude/hooks/no_compound_bash.py`) is committed,
but its `PreToolUse` **wiring** is not. To turn the guard on, add this
`PreToolUse` hook to your `.claude/settings.json`:

<!-- markdownlint-disable MD013 -->

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "python3 \"$CLAUDE_PROJECT_DIR/.claude/hooks/no_compound_bash.py\"",
            "statusMessage": "Checking for compound shell",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

<!-- markdownlint-enable MD013 -->

Baseline permission allow-rules (the `Bash(prefix:*)` globs the shell
rules produce) go in the same file, or in `settings.local.json` — the
`firm-perms` skill maintains the local allowlist for you. See
"How settings files resolve across worktrees" below for *which* file a
worktree session actually reads and writes: it is one shared file, not
a per-worktree copy.

## How settings files resolve across worktrees

**`.claude/settings.local.json` is one shared file, resolved through
worktrees to the main checkout.** Per the official Claude Code docs
(`settings.md`, `permissions.md`, `worktrees.md`), it is read **and
written** "at the root of the git repository, resolved through
worktrees to the main checkout". So:

- A worktree checkout carries **no copy of its own** — verify with
  `ls .claude/` in any worktree; there is nothing there.
- A **"don't ask again" approval inside a worktree session saves to
  the main checkout's file**, and is therefore live in every other
  worktree immediately.
- The `hooks` key resolves the same way. **Guard hooks wired in the
  main checkout's `settings.local.json` do fire in worktree
  sessions** — verified 2026-08-14 by controlled probe: a worktree
  with no settings file of its own, whose only wiring was the base
  repo's, had a deliberate `echo a && echo b` blocked by
  `no_compound_bash.py`.

`permissions.md`'s line about settings keys loading from the cwd's
`.claude` with "no parent-directory fallback" describes **directory
nesting** — it does not override worktree-to-main-checkout resolution,
which is a distinct, explicitly documented mechanism.

**This corrects a superseded model.** An earlier version of this doc
claimed a worktree "does not inherit the base repo's copy
automatically" and that `firm-perms`' sweep was what propagated it.
That was wrong: there is nothing to propagate, because there is only
one file. Two consequences follow, both fixed in the skills:

- `firm-perms`' worktree-plus-base **dual-write is redundant by
  design** — the worktree write already lands in the main checkout's
  file.
- "User scope is the only thing a fresh worktree inherits" is
  **false**. The real criterion for putting a rule in
  `~/.claude/settings.json` is **cross-*repo* portability** — a rule
  you want in *other* projects too — not worktree inheritance.

What a fresh worktree genuinely lacks is anything *untracked and
per-directory*: `frontend/node_modules`, `frontend/.env.local`, and
`infra/localnet/secrets.local.env`. Those are `init-pr`'s job, and are
unrelated to settings resolution.

The last of those is worth calling out, because the resolution rule
above does **not** cover it. The secrets enclave's operator file is a
plain per-checkout path — nothing resolves it back to the main checkout
— so a fresh worktree has none, and `make collectors-up` there brings
up the keyless feeds and warns that it is skipping the keyed venues.
`init-pr`
symlinks it from the base repo (`init_pr_branch.py --link-env`,
reported as `secrets_env_link`), which is what gives that file the
resolution `settings.local.json` gets for free. Do not generalize from
one to the other.

### Which guards are actually wired

Reach was never the problem — **wiring** is. All **four** guard scripts
are committed under `.claude/hooks/`, but a script only runs if a
`PreToolUse` entry points at it, and a committed-but-unwired guard is a
documented protection that does not exist. That has happened: as of
2026-08-14 the main checkout wired **only** `no_compound_bash.py`,
leaving `no_git_grep.py` and `worktree_edit_guard.py` inert — including
the worktree edit-path guard, which exists specifically for worktree
sessions. `no_destructive_bash.py` is the newest and starts unwired
like the rest. Wire the ones you want using the blocks in each guard's
section below.

Don't trust that paragraph's date — **ask**, since the answer is
per-machine and changes the moment someone edits a git-ignored file:

```sh
make hook-wiring
```

That is the authority on which guards are live here; the prose above is
only the finding that prompted the check.

## The git-grep guard hook

The [shell-commands](shell-commands.md) rule "never `git grep`" is also
enforced **mechanically**. A `PreToolUse` Bash hook
(`.claude/hooks/no_git_grep.py`) inspects each Bash command and
**blocks** any that runs `git grep` — including `git -C <path> grep`,
`git --no-pager grep`, `git -c core.pager=cat grep`, `--flag=value`
global-option variants, and a `git grep` that follows a shell control
operator (`&&` / `|` / `;`). It nudges the model to the **Grep tool**
(or a bare `grep` where Grep is absent). The scan is **quote-aware**
and the guard fails *open* — any payload it can't parse is allowed — so
it never wedges a session.

Why a guard beats allow-listing `git grep`: a plain `git grep foo`
re-prompts until firmed, and a quoted `\|` alternation trips the
harness's per-subcommand `|` guard so it **can't be firmed at all** —
allow-listing would leave the alternation broken and might *increase*
`git grep` use. The Grep tool sidesteps both (real regex, cross-path,
zero prompts), so — unlike the compound guard — this hook has **no
escape hatch**.

That is deliberate, not an oversight, but it does cost one real
capability: **revision-scoped** content search, which the Grep tool
structurally cannot do. The full adjudication — why a rev-vs-pathspec
carve-out can't be made guard-safe, the indirect workarounds
(`git log -S` / `-G`, `git show <rev>:<path>`, a throwaway
`git worktree add --detach` plus the Grep tool), and the pre-registered
trigger for building a tool instead of a carve-out — is in
[shell-commands](shell-commands.md#why-the-git-grep-ban-stays-absolute).

### Wiring the git-grep guard

Like the compound guard, the **script** (`.claude/hooks/no_git_grep.py`)
is committed with a built-in self-test —
`python3 .claude/hooks/no_git_grep.py --self-test` — but its
`PreToolUse` **wiring** is not. It shares the `Bash` matcher with the
compound guard, so add its `command` entry alongside that guard's under
the same matcher (matching `PreToolUse` hooks all run, order-independent):

<!-- markdownlint-disable MD013 -->

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "python3 \"$CLAUDE_PROJECT_DIR/.claude/hooks/no_compound_bash.py\"",
            "statusMessage": "Checking for compound shell",
            "timeout": 10
          },
          {
            "type": "command",
            "command": "python3 \"$CLAUDE_PROJECT_DIR/.claude/hooks/no_git_grep.py\"",
            "statusMessage": "Checking for git grep",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

<!-- markdownlint-enable MD013 -->

## The destructive-command guard hook

The two guards above check shell **form**; the worktree guard below
checks edit **path**. None checks command **danger**, and that is the
failure discovered latest and most expensively — a recursive delete or
a force-push is noticed after it has run.

`.claude/hooks/no_destructive_bash.py` closes that gap in **two
tiers**:

- **ASK** — a recursive force-delete (`-r` or BSD `-R`), a force-push
  (flag-first, flag-last, or a `+src:dst` refspec), a hard reset, a
  `git clean` that removes untracked files, destructive SQL (`DROP`,
  `TRUNCATE`, a `DELETE` with no `WHERE`), a docker prune or volume
  removal, a forced branch delete. Blocked with a message naming what
  tripped, and **overridable** with the literal marker
  `#destructive-ok` in the command — so a deliberate one stays
  possible and stays auditable in the transcript.
- **DENY** — a very small catastrophic set that **no marker lifts**: a
  recursive delete of `/`, `~`, or `$HOME` (bare, trailing-slash or
  globbed), and a force-push to the default branch in any of its
  spellings.

**Two places the coverage is deliberately narrow, so it is not
over-read:**

- **Destructive SQL is recognized only when a SQL client is named on
  the same line** (`psql`, `mysql`, `sqlite3`, `sqlx`, `pg_dump`,
  `cockroach`, `clickhouse`). Un-gated, the patterns matched ordinary
  English and blocked ordinary commit messages: a subject line of
  `Drop table borders` tripped it. A guard that blocks `git commit` is
  a guard that gets turned off, the worse security outcome. The cost is
  that a
  heredoc or a piped `echo … | psql` puts the keyword on a different
  line from the client and is not seen; two of those three shapes are
  already barred by the compound guard.
- **`git clean` is exempted only by a real dry-run flag** — a short
  cluster containing `n`, or `--dry-run`. A looser test read any flag
  containing an `n` as a preview, so an ordinary
  `--exclude=node_modules` made a real `git clean -fdx` look like a dry
  run and go unclassified. This is the failure mode to watch for across
  this whole file: **a false-positive fix opening a real hole.**

Two implementation notes worth keeping, because both were found by the
script's own self-test rather than in review:

- **The tier is decided before the marker is read.** Reading the
  escape hatch first would hand the deny tier an override. The
  self-test pins this.
- **The command is classified with its trailing comment removed.** A
  shell comment is inert, so this is faithful — and it closes a real
  bypass: the deny patterns anchor the target path at end-of-command,
  so `rm -rf / #destructive-ok` failed to match deny, fell through to
  ask, and was then lifted by the very marker deny must ignore.
- **Lines are classified one at a time, but a trailing backslash is
  collapsed first.** Newline is a command separator, so a multi-line
  payload is several commands and must not be matched as one blob —
  except after a `\`, where the newline is *not* a separator and
  splitting on it cut `rm -rf \` away from its `/` target, dropping a
  deny to a liftable ask.

**It is a best-effort advisory stop, not a policy boundary.** It
matches patterns over one command string; it is not a sandbox, and an
unusual spelling gets through. Its job is to catch the slip, and it is
worth having on exactly that basis.

Self-test: `python3 .claude/hooks/no_destructive_bash.py --self-test`.
Wire it under the same `Bash` matcher as the two above:

<!-- markdownlint-disable MD013 -->

```json
{
  "type": "command",
  "command": "python3 \"$CLAUDE_PROJECT_DIR/.claude/hooks/no_destructive_bash.py\"",
  "statusMessage": "Checking for destructive commands",
  "timeout": 10
}
```

<!-- markdownlint-enable MD013 -->

## The worktree edit-path guard hook

In a worktree session the build and tests run against the *worktree*
checkout, so editing a file through its **base-repo absolute path**
(`/…/dropset/foo.rs`) instead of the worktree path
(`/…/dropset/.claude/worktrees/<tag>/foo.rs`) writes to a copy the
worktree build never sees — a new test "doesn't appear," a fix "doesn't
take," and the slip surfaces only after a wasted rebuild. It is a
recurring, expensive mistake. A `PreToolUse` guard
(`.claude/hooks/worktree_edit_guard.py`) catches it at the tool call:
when the active checkout is a worktree and a **file-mutating** tool
(`Edit` / `Write` / `MultiEdit` / `NotebookEdit`) targets a base-repo
absolute path, it **blocks** and names the worktree-local path to use
instead. A `Read` of a base path is merely wasteful, not corrupting, so
it is left alone.

Two carve-outs pass through: the base `.claude/settings.json` /
`settings.local.json` files (which `firm-perms` and `firm_last.py` write
on purpose), and the env escape `ALLOW_BASE_REPO_EDITS=1` for a rare
deliberate base edit. The guard fails *open* — a missing field or parse
problem is allowed — so it never wedges a session, and relative paths
(which resolve against the worktree cwd) are always allowed.

Like the compound guard, the **script** is committed with a built-in
self-test —
`python3 .claude/hooks/worktree_edit_guard.py --self-test` — but its
`PreToolUse` **wiring** is not committed. To turn it on, add to your
`.claude/settings.json`:

<!-- markdownlint-disable MD013 -->

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit|NotebookEdit",
        "hooks": [
          {
            "type": "command",
            "command": "python3 \"$CLAUDE_PROJECT_DIR/.claude/hooks/worktree_edit_guard.py\"",
            "statusMessage": "Checking edit target",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

<!-- markdownlint-enable MD013 -->

## iTerm2 tab-color integration

A set of shell scripts under `.claude/scripts/` drive the color of the
iTerm2 tab (and, optionally, the window background) from Claude Code
hooks, so a glance at the tab strip tells you which session needs you.

### What the tab signals

- **Green** — Claude wants a reply: it is done (a `Stop`), or it is
  asking you a question (the `AskUserQuestion` tool). Go respond.
- **Yellow** — Claude needs an approval to keep going: a harness
  permission prompt, or it is about to edit a file. Go approve.
- **No tint** — working, or you acknowledged it with the attend
  shortcut.

`AskUserQuestion` (a tool, so green) and a permission prompt (a
harness-native dialog, so yellow) are *different mechanisms*, which is
why they get different colors — see "How the color is chosen" below.

iTerm mutes the color of a non-selected tab and there is no setting to
stop it, so the tints are picked bright enough to stay legible when the
tab isn't focused.

### The scripts

All live in `.claude/scripts/` and are dependency-free bash:

- `iterm-colors.sh` — shared palette and the SetColors emit helpers,
  sourced by the rest. The four states and the `PAINT_WINDOW_BG` toggle
  (off by default: only the tab is tinted) live here; edit this file to
  recolor everything.
- `iterm-paint.sh` — the hook painter. Every hook calls this one script;
  it reads the hook event on stdin and picks the color itself (see
  below).
- `iterm-monitor.sh` — a per-TTY daemon that applies the state color and
  continuously suppresses iTerm's own attention/badge requests. It
  re-reads the palette on each state change, so edits to
  `iterm-colors.sh` apply to a running session without a restart.
- `iterm-start.sh` / `iterm-stop.sh` — SessionStart / SessionEnd:
  start/stop the monitor and set the initial / cleared state.
- `iterm-attend.sh` — the "attend" toggle (bound to a keyboard
  shortcut): flips the tab between the green mark and neutral, like
  mark-as-unread. Two optional flags let a caller mark a tab it does
  **not** live in: `--tty <path>` targets another session, and `--mark`
  sets green outright instead of toggling. The fleet-resume launcher
  uses both — a coprocess bound to a key can only reach its own session,
  and a launcher wants green rather than "the other one".
- `iterm-restart-monitors.sh` / `iterm-reset-windows.sh` — recovery
  sweeps (see "Recovery" below).
- `iterm-reorder.py` — the FIFO tab-reorderer (see "FIFO attention
  ordering" below). Python, not bash, because reordering is only possible
  through iTerm2's Python API.

Per-TTY state and pid files live under `/tmp/iterm-color-<tty>` and
`/tmp/iterm-monitor-<tty>.pid`; the shell-registered session→tty map
lives under `/tmp/iterm-session-tty-<uuid>`. Those prefixes are defined
once in `iterm-colors.sh`.

### How the color is chosen

The painter is the single decision point, and this is deliberate.
**Matching `PreToolUse` hooks run in parallel with no ordering
guarantee**, so the earlier design — one hook painting neutral on `*`
alongside a second painting green on `AskUserQuestion` — was a race, and
the tab color was non-deterministic. Collapsing every event to *one*
hook that calls `iterm-paint.sh`, which then derives the color from the
event on stdin, makes the color a deterministic function of the event:

- `PreToolUse` with `tool_name` `AskUserQuestion` → green; with an edit
  tool (`Edit` / `Write` / `MultiEdit` / `NotebookEdit`) → yellow;
  anything else → neutral.
- `Notification` with `notification_type` `permission_prompt` → yellow;
  any other notification leaves the tab unchanged.
- `Stop` → green. `PostToolUse` / `UserPromptSubmit` → neutral.

`AskUserQuestion` **does** fire a companion `Notification`
(`permission_prompt`) — from the harness's side the selector is a block
on user input, which looks like a permission — so the raw event mapping
above would paint the tab yellow right after the tool's green, by
last-write. The `permission_prompt` payload carries nothing that tells
that companion apart from a genuine tool-permission prompt, so the
painter makes the `AskUserQuestion` green **sticky**: painting it drops a
per-tty sentinel (`/tmp/iterm-color-<tty>.askq`), and **every**
`permission_prompt` `Notification` is suppressed while that sentinel is
present. The harness **re-fires** that notification periodically while
the selector waits, so the suppression must last until the selector is
answered (its `PostToolUse`) or any other paint clears the sentinel —
there is deliberately **no time window** (a fixed window let a re-fired
notification repaint yellow mid-wait). The sentinel is cleared on any
other paint, and a stale one from a crashed session is dropped at session
start, so a genuine permission prompt that follows unrelated work still
turns the tab yellow.

### FIFO attention ordering

Beyond coloring, `iterm-reorder.py` keeps each window's tabs sorted into
attention groups so you can park at position 1 and sweep right:

```txt
[ yellow (permission) … ] [ green (reply wanted) … ] [ everything else … ]
```

Within each attention group the order is **FIFO**: the tab that has
waited longest stays leftmost, and a tab that newly needs attention goes
to the *back* of its group (just before the next group). So position 1 is
always the longest-waiting item — clear it, it drops below all attention
tabs, and the next-oldest slides into position 1.

The reorderer never steals focus: `async_set_tabs` preserves the selected
tab, so a tab you're working in slides to its queue position but stays
focused until *you* navigate away (e.g. `Cmd-1` to jump to the
longest-waiting item).

Reordering a tab is **only possible through iTerm2's Python API**
(`window.async_set_tabs`) — no escape sequence moves a tab — so this half
of the integration is a Python daemon, separate from the per-TTY color
hooks. It reads the same `/tmp/iterm-color-<tty>` state the hooks write,
maps each tab to it via the session's `tty` variable, and tracks the FIFO
sequence itself. The pure ordering (`plan_order`) is unit-tested under
`make tools-tests`; the live reordering can only be exercised against a
running iTerm2.

To run it: enable the API (Prefs → General → Magic → **Enable Python
API**), then run `iterm-reorder.py` as a long-lived script — either drop
it in `~/Library/Application Support/iTerm2/Scripts/AutoLaunch/` (iTerm2
provisions its `iterm2`-package venv and launches it at startup) or run
it by hand in a venv that has the `iterm2` package. It is additive: the
color hooks keep working whether or not the reorderer is running.

### Wiring the hooks

Add to your `~/.claude/settings.json` (alongside the compound guard
above). Every event routes to the one painter, except SessionStart /
SessionEnd which manage the monitor:

<!-- markdownlint-disable MD013 -->

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.claude/scripts/iterm-start.sh\"" } ] }
    ],
    "PreToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.claude/scripts/iterm-paint.sh\"" } ] }
    ],
    "PostToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.claude/scripts/iterm-paint.sh\"" } ] }
    ],
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.claude/scripts/iterm-paint.sh\"" } ] }
    ],
    "Notification": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.claude/scripts/iterm-paint.sh\"" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.claude/scripts/iterm-paint.sh\"" } ] }
    ],
    "SessionEnd": [
      { "hooks": [ { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.claude/scripts/iterm-stop.sh\"" } ] }
    ]
  }
}
```

<!-- markdownlint-enable MD013 -->

Hook changes only take effect in a **new session** — edit the file, then
start a fresh Claude Code session to pick them up.

### Shell setup (`~/.zshrc`)

The attend toggle needs a stable session→tty map, and the Linear
automation needs its ids in the environment. Put all of it in one place:

- **Linear MCP ids** — `LINEAR_TEAM_ID`, `LINEAR_PROJECT_ID`, and
  `LINEAR_ASSIGNEE_ID` (see
  [linear-automation](linear-automation.md) for what reads them).
  These are plain identifiers, so they sit in the file directly.
  `LINEAR_API_KEY` is a secret and is resolved separately — see
  "Session secrets" below.

- **Session→tty registration**, keyed by the *stable* session UUID:

  ```sh
  if [ -n "$ITERM_SESSION_ID" ]; then
    tty > "/tmp/iterm-session-tty-${ITERM_SESSION_ID##*:}"
  fi
  ```

  Key by the UUID only (`${ITERM_SESSION_ID##*:}`), **not** the whole
  `$ITERM_SESSION_ID`: its leading `wNtNpN` window/tab/pane prefix drifts
  when a pane is moved or split, so a coprocess launched later would look
  under a different key than the shell registered. Keep this rationale
  so the line isn't "simplified" back to the full id.

- **`DISABLE_AUTO_TITLE=true`** — stops the shell from re-titling the
  tab out from under the integration.

### Session secrets

`LINEAR_API_KEY` and `GITHUB_MCP_PAT` are secrets, so unlike the ids
above they are never written into a config file. A `_ds_secrets` helper
resolves them from 1Password, and every session-*starting* helper calls
it before launching Claude Code (`cdds` does not — it only changes
directory).

**The function is committed; its coordinates are not.** `_ds_secrets`
lives in `.claude/shell/init.zsh` with the rest of the family, and reads
the account, vault, and item names from three shell variables:

```sh
DS_OP_ACCOUNT='<account>.1password.com'
DS_OP_LINEAR_REF='op://<vault>/<linear-item>/credential'
DS_OP_GITHUB_REF='op://<vault>/<github-item>/credential'
```

**Define those in the untracked runtime config** (`~/.zshrc`), alongside
the `LINEAR_*` ids that already live there. **One personal config file,
not two** — that is the operator decision, and the reasoning is that the
separate coordinates file existed to keep *scripts* out of the profile,
so with the function bodies now committed there is nothing left for it
to keep out.

The committed function reads whatever `DS_OP_*` variables are
shell-visible, so plain assignments in the profile satisfy it directly.

That split — committed function, uncommitted coordinates — is the point
of the boundary. An `op://` reference is a *pointer*, not a value, so
committing one would leak no credential; but it would publish the layout
of a personal secret store into permanent git history, and history does
not forget.

*Deliberately one location only.* An earlier revision also sourced a
separate file at `~/.config/dropset/secrets.zsh` (or wherever
`DROPSET_SECRETS_FILE` pointed). That path was removed rather than left
dormant: two supported homes for the same three variables is somewhere
for them to disagree, and the failure is silent — the stale copy wins
and the wrong credential resolves. The profile is the one home.

With no coordinates set at all the helper resolves nothing and warns; an
already-exported `LINEAR_API_KEY` / `GITHUB_MCP_PAT` still wins, so
pinning a key by hand remains the escape hatch.

**Migrating off an in-profile copy — a one-time operator step.** Before
the helper family was committed, `~/.zshrc` defined its **own**
`_ds_secrets` function. Sourcing `.claude/shell/init.zsh` does **not**
remove that older definition, and whichever is defined last wins — so a
profile still carrying its own copy silently overrides the committed
version, which is the real hazard here. Two checks say whether the
migration is done, and note that **neither is a count of `op://`
references**: under the shape above the profile legitimately carries the
two coordinate refs.

- **No personal `_ds_secrets` function definition remains** in the
  profile — only the single guarded line that sources `init.zsh`.
- **No direct exports of `LINEAR_API_KEY` or `GITHUB_MCP_PAT`.** A
  *secret* assigned in a config file is the thing this boundary exists
  to prevent; a `DS_OP_*` *pointer* is not.

Nothing in the repo can do this step — `~/.zshrc` is untracked and
machine-local by design — which is exactly why it is written down here
rather than automated.

Four things about that shape are load-bearing:

- **Resolution is lazy — at session launch, not at shell init.**
  `op read` costs a round trip and can raise a Touch ID prompt, and
  every plain terminal tab would otherwise pay both for secrets it will
  never use. Only the session helpers call `_ds_secrets`, so opening an
  ordinary tab stays instant.

- **The `${VAR:-…}` guard makes it at most one fetch per shell.** A
  second call in the same shell is a no-op, so helpers that chain into
  one another don't re-prompt. It also lets an already-exported value
  win, which is the override path when a key has to be pinned by hand.

- **`--account` is explicit** because the laptop is signed into more
  than one 1Password account, and a bare `op read` cannot disambiguate
  between them.

- **An unresolved secret warns rather than failing the launch.** An
  empty key otherwise surfaces much later as an opaque MCP error
  mid-session, which is far worse to debug than one warning line at
  startup.

The coordinates above are placeholders. The real account domain, vault
name, and item titles appear only in the untracked runtime config —
never in a symlink into a tracked config repo — so substitute your own.
Naming the real ones here would buy a reader nothing (they have to
substitute regardless).

### Session helpers (`.claude/shell/init.zsh`)

The function family that starts and resumes Claude Code sessions is
**committed**, at `.claude/shell/init.zsh`. The shell profile's whole
share of it is one guarded line:

```sh
[[ -r ~/repos/dropset/.claude/shell/init.zsh ]] &&
  source ~/repos/dropset/.claude/shell/init.zsh
```

Source the **base checkout's** copy, mirroring how `settings.local.json`
resolves: exactly one live version exists and worktree copies are inert.
The script says so itself — it derives the repo root from its own
sourced location and warns if that lands inside `.claude/worktrees/`,
because a worktree copy would otherwise make every helper treat that
worktree as the base repo, quietly and plausibly. Guard the line so a
moved checkout costs a no-op rather than a broken shell.

**This replaces hand-copying.** These functions previously existed only
as reference implementations in this doc, which the operator copied into
an untracked `~/.zshrc` — the same failure class as a guard hook with no
wiring: documented, executable nowhere, and drifting with nobody able to
see the drift. It was not hypothetical; the `paps` block published here
never worked (below). Committing the functions makes this doc describe
something that actually runs.

**And the drift ran the other way too — check parity when committing a
personal helper.** The committed family was written from this doc rather
than from the profile, and the profile had moved on: the operator's own
`aps` passed `acceptEdits`, a display name, and `/init-pr`, its `naps`
passed a permission mode, and its base-`cd` did a `git pull` — none of
which the committed copies did. The gap went unnoticed for a while
because every helper still *worked*; it was only the launch *shape* that
differed, and the `acceptEdits` one meant sessions started via the
committed `aps` prompted on every edit. Worse, the migration that
revealed this found the profile had **never sourced** the committed
script at all — it still carried full personal copies of every helper,
so nothing committed here had ever run.

The lesson generalizes past this one file: when moving an operative
personal script under version control, diff the **behavior**, not just
the shape, and decide each difference deliberately rather than
inheriting it.

**And check that a deliberate decision was the operator's to make.**
The `cdds` pull is the cautionary case: the migration decided *against*
adopting it, recorded the reasoning here, and was later reversed by
operator direction — the pull is now in `_ds_pull` and every verb
inherits it. The reasoning was sound in isolation (a navigation command
making a network call is a real cost) and still wrong, because it traded
away a property the operator was relying on: entering the repo means
having current code. A behavior the operator's own copy had is evidence
of intent, not merely a difference to adjudicate — so the burden runs
the other way, and dropping one wants an explicit ask rather than a
recorded rationale.

**What cannot ride this file:** the guard hooks' `settings.json` wiring.
That is JSON read by the harness, not shell read by zsh, so sourcing
this changes nothing about it — `make hook-wiring` remains the answer
there.

The family, one line each (`init-pr` names `aps` when it explains why a
branch arrives as `worktree-eng-###`):

- **`_ds_pull`** (internal) — fast-forward the **base** checkout's
  `main`. Everything below inherits it, via `cdds` or via the internal
  `_ds_base`, because the standing direction is that entering the repo —
  or entering a worktree *from* the repo — means having current code.
  Four properties carry the design:

  - **Fast-forward only.** It never merges or rebases local work; a
    divergence is reported and left for the operator.
  - **The base checkout only, never a worktree.** A worktree's checkout
    is a work branch, so pulling inside it would mutate that branch.
    Worktrees share the object store, so fast-forwarding the base is
    exactly what makes fresh `origin` refs reachable from them — the
    benefit at none of the risk. On any branch but `main` it fetches and
    stops.
  - **Quiet on success, loud on trouble, fatal never.** A dead network,
    an expired credential or a diverged `main` warns and returns 0. A
    navigation command must not be able to brick session startup.
  - **Throttled** (60s, stamped in `.git/.ds-last-pull`, claimed
    *before* the pull). Not an optimization: `faps go` opens many tabs
    at once and a pull takes `index.lock`, so without the throttle they
    race and print git errors over each other.

- **`cdds`** — `cd` to the base repo checkout. The starting point for
  anything that must not run inside a worktree (`housekeeping`, a
  planning session). **It fast-forwards `main` on the way in**, through
  the shared `_ds_pull` helper described below. With a tag
  (`cdds 1077`), the pull happens in the base repo *before* the `cd`, so
  the worktree gets fresh `origin` refs through the shared object store
  without its work branch being touched.

- **`aps <tag>`** — start a **worktree** session:
  `claude -w <tag> -n <tag> --permission-mode acceptEdits /init-pr`.
  This is what creates the `eng-###` worktree directory whose branch
  arrives named `worktree-eng-###`; there is no CLI flag to drop the
  prefix, so `init-pr` renames it. This is the implementation-session
  entry point. A **bare number** is given the `eng-` prefix, so
  `aps 882` and `aps eng-882` agree and the `aps` → `raps` pair
  composes; a deliberate non-`eng` name passes through untouched.

  **`--permission-mode acceptEdits` is the load-bearing part.** The
  shared `settings.local.json` sets no default permission mode, so
  without it an implementation session starts in the default mode and
  prompts on every edit. `-n <tag>` is for the human (the prompt box,
  the `/resume` picker, the terminal title — `raps` resolves by
  directory), and `/init-pr` runs the bootstrap without being asked, the
  same trick `paps` and `haps` use.

- **`raps <n>`** — resume a worktree session by number: takes a bare
  `<n>`, resolves it to the `eng-<n>` worktree, and resumes that
  session's most recent conversation there. The number-to-worktree
  resolution is the whole point — you resume `raps 814`, not a UUID.

- **`naps <name>`** — start a **named** session in the current directory
  (no worktree), also with `--permission-mode acceptEdits` for the reason
  above. The general-purpose named-session entry point.

- **`rnaps <name>`** — resume a named session by the same name. The
  counterpart to `naps`, as `raps` is to `aps`; added so a long-running
  session survives a closed terminal. **A bare name pre-filters the
  interactive picker rather than resuming deterministically** —
  `-r/--resume` matches on session *ID*, and a name is not one, so
  expect to pick from a list. That is the same underlying fact that made
  the old `paps` wrong, and it is why `paps` / `haps` compute an id of
  their own instead.

- **`paps`** — start **or resume** a **planning** session. Takes no
  argument: it derives the session name `plan-<day-of-month>` from
  today's date (run on the 14th → `plan-14`), `cd`s to the base repo,
  and launches Claude Code with `--model claude-fable-5` and `/plan`
  as the initial prompt.

  `paps` is **idempotent by design** — if today's `plan-<day>` session
  already exists it resumes it, otherwise it creates it. That collapses
  the new-vs-resume split into one verb, which is the point: a planning
  session is opened and reopened many times in a day, and having to
  remember which state it is in is the friction the helper removes. An
  `rpaps` twin was considered and rejected for that reason.

  Three things it makes deterministic, each of which used to be a
  manual step the operator could forget:

  - **The model.** Planning sessions run the most capable model
    deliberately — fidelity over tokens — and passing `--model` at
    launch is the only session-wide mechanism. Skill frontmatter
    (`model: fable` on the `plan` skill) is belt-and-braces for a
    mid-session `/plan`, not a substitute; whether it switches the
    session going forward is unspecified.
  - **The directory.** A planning session touches the board, not a
    branch, so it must run in the base repo. `paps` `cd`s there
    itself rather than trusting the shell's cwd.
  - **The bootstrap.** Passing `/plan` as the initial prompt means the
    skill's bootstrap read happens without being asked for.

  This **supersedes** `naps planning-<day>` / `rnaps planning-<day>`
  and the older `planning-<day>` session naming. `naps` / `rnaps`
  remain, for named sessions that aren't planning sessions.

  `date +%-d` gives an unpadded day, so the 5th is `plan-5`, not
  `plan-05`.

- **`haps`** — start **or resume** today's **housekeeping** session, so
  a day's upkeep is one verb rather than a hand-started session. Same
  contract as `paps`, with three substitutions: the display name is
  `housekeeping-<day-of-month>`, the initial prompt is `/housekeeping`,
  and the session-id seed carries its own prefix so a day's planning and
  housekeeping sessions cannot collide. **No model pin**, deliberately —
  housekeeping is upkeep, not board decisions, so it does not inherit
  the planning tier and runs on the saved default.

- **`caps <topic>`** — start **or resume** an **architect** session on
  one topic: the CEO hat, and the complement to `paps` rather than a
  variant of it. Same base-repo launch, same model pin, same
  idempotency; the different half is the briefing (`/architect`) and
  what it may write — **nothing to the board**.

  It keys on the **topic, not the date**, which is the one substantive
  difference from `paps` / `haps`: a design thread outlives a day, so
  `_ds_topic_sid` omits the date from the seed. Putting the date in
  would silently start a fresh conversation each morning and lose the
  thread — the failure the verb exists to prevent. Each topic gets its
  own resumable session, so parallel threads never share context.

  The display name is `ceo-<topic>`, which makes the fleet listing read
  by role: `eng-*` implementers, `plan-*` planning, `ceo-*`
  architecture. The topic is validated to lowercase letters, digits and
  dashes, since it reaches both a session name and a filename.

  All three standing verbs share one core, `_ds_session`, which takes
  an already-computed id — `paps` and `haps` hand it a daily id,
  `caps` a topic id. That split is deliberate: the operator's stated
  abstraction is that these launchers differ **only in the briefing**.

- **`faps [go]`** — resume the whole **fleet**: one iTerm tab per
  in-flight Linear issue, each with `raps <n>` typed **and Enter
  pressed**, and each flagged green for attention. The batch counterpart
  to `raps`, for after a machine restart — the loaded window becomes a
  to-attend list with nothing left to type.

  A bare `faps` prints the plan and opens nothing; `faps go` applies it.
  Read-only by default deliberately: one verb can open many tabs and
  resume many sessions, so seeing the list first is worth a word.

  Three things it gets right, and each is a reason it is a tool rather
  than a shell one-liner:

  - **In-flight means state *type* `started`**, not the state names —
    which covers **In Progress and In Review**, and In Review is
    load-bearing since a merged PR with outstanding follow-up stays
    there (see `linear-automation.md` → "The Linear state tracks the
    SESSION, not the PR"). Matching the type means a workflow rename
    cannot silently drop a session from the fleet.
  - **It skips an issue whose tab is already open**, so it is safe to
    run twice. The signal is the iTerm tab *name*, which carries the tag
    because `aps` passes `-n <tag>` — so the two are coupled: if `aps`
    ever stops setting a display name, this stops recognizing live
    sessions and starts double-resuming.
  - **The attention mark is driven from outside the tab.** A coprocess
    bound to a key can only ever reach its own session, so the launcher
    reads each new tab's `tty` out of AppleScript and calls
    `iterm-attend.sh --tty <path> --mark`. `--mark` *sets* green rather
    than toggling: a toggle's outcome depends on the tab's history, and
    a launcher wants green, not "the other one".

  The deterministic half — the Linear query, the tag derivation, the
  already-live check, the emitted AppleScript — is the committed tool
  `.claude/tools/fleet_resume.py`; `faps` is the thin verb over it, per
  the skill-tooling convention. **The apply path's effect is not
  unit-testable** (asserting it would mean opening tabs in a live window
  and resuming real work sessions), so the emitted script is verified by
  compiling it against iTerm's scripting dictionary with `osacompile`,
  which resolves every term and executes nothing.

#### Why `paps` and `haps` compute a session id

The idempotency is **forced, not over-engineered**, and the reasoning is
worth recording because the obvious implementation is the one that was
published here and never worked. Verified against `claude --help`:

- There is **no session-listing flag**, in any spelling. The block this
  doc used to carry probed `claude --list-sessions`, and hedged with
  "adjust the existence probe to whichever form your CLI supports" —
  pointing at a dead end, since none does.
- **`-n/--name` sets only a *display* name** (prompt box, `/resume`
  picker, terminal title). Nothing resolves a session *by* it.
- **`-r/--resume` takes a session ID.** A bare string opens an
  interactive picker filtered by that term, so the old block's
  `claude --resume "$name"` was non-deterministic too — both halves
  were wrong, not just the probe.

So the only deterministic handle is an id the helper computes itself: a
**per-day session UUID** seeded from kind + full date, with the on-disk
transcript as the existence check. The seed carries the **full** date so
`plan-18` in August and `plan-18` in September cannot collide — the
display name stays day-only by operator choice, and the id is what
disambiguates. The transcript-path slug replaces every `/` and `.` with
`-`, the same rule `.claude/tools/firm_last.py`'s slugify encodes.

The lesson generalizes past this one block: **a committed code block
that invokes a CLI flag is checkable against that CLI's `--help`**, and
this one would have been caught at filing time rather than at wiring
time.

The split is deliberate and matches the session kinds: worktree
sessions (`aps` / `raps`) run one deterministic spec to completion and
are addressed by their Linear number; the standing sessions (`paps`,
`haps`) run in the base repo, recur daily, and are addressed by the day
they started.

### iTerm2 manual setup (can't be committed)

Some of this lives only in iTerm2's own preferences:

- **The attend shortcut.** Prefs → Keys → Key Bindings → add a binding
  whose action is **Run Coprocess…**, pointing at the **absolute path**
  of `iterm-attend.sh` in your deployed scripts dir (e.g.
  `~/.claude/scripts/iterm-attend.sh`). (A coprocess inherits
  `$ITERM_SESSION_ID`, which is why the attend script can resolve its tty
  from the registration above.) **This binding is a stored absolute path,
  so renaming or moving the script silently breaks it** — a coprocess
  aimed at a missing file throws an error on the keypress. If you
  migrated from an earlier script family (see "Deploying to `~/.claude`"
  below), **repoint this binding** to the new `iterm-attend.sh`; it is
  not updated by copying the new scripts in.
- **Drop the job-name title suffix.** Profiles → General → Title →
  uncheck **Job Name**, so the tab title stops showing the `(python)`
  suffix of the running process.
- **iTerm2 only.** The integration uses iTerm2's proprietary `SetColors`
  and `RequestAttention` escape sequences; they silently no-op in other
  terminals, so the coloring simply does nothing elsewhere (it doesn't
  break anything).
- **Bright tints are intentional.** iTerm mutes the color of an inactive
  tab and offers no setting to disable that, so the palette is picked
  bright enough to read while muted.

### Deploying to `~/.claude` (and migrating the script family)

The wiring above uses `$CLAUDE_PROJECT_DIR`, which resolves to the active
checkout — convenient, but it only colors sessions *inside a checkout
that has these scripts*. To get the coloring in **every** Claude Code
session regardless of directory, deploy the integration **globally**:
copy the `iterm-*.sh` scripts (and `iterm-reorder.py`) into
`~/.claude/scripts/`, and wire the hooks in `~/.claude/settings.json`
using **absolute** `~/.claude/scripts/…` paths instead of
`$CLAUDE_PROJECT_DIR`. The reorderer goes live per "FIFO attention
ordering" above (drop `iterm-reorder.py` in the iTerm2 `AutoLaunch/`
folder; it needs iTerm2's **Python Runtime** installed — Scripts →
Manage → Install Python Runtime — and the API enabled).

Migrating from an **older, differently-named** script family (a rename,
e.g. an `iterm-bg-*` set) has four gotchas, none of which a plain
file-copy handles:

1. **Remove the old scripts** — a leftover family just confuses.
1. **Rewire `settings.json`** to the current single-painter shape (one
   `PreToolUse` `matcher: "*"` → `iterm-paint.sh`), not the old
   multi-matcher wiring — the painter derives the color from the event,
   so parallel matchers are a race (see "How the color is chosen").
1. **Hook changes load only in a new session** — the running session
   keeps the old wiring until you start a fresh one.
1. **Repoint the attend key binding.** The Cmd-Shift-A "Run Coprocess"
   binding stores an **absolute script path** in iTerm2's prefs; a
   rename leaves it aimed at the deleted script, so the keypress throws
   a coprocess error until you repoint it (see "The attend shortcut"
   above). Copying the new scripts does **not** fix it.

### Recovery

The monitor is long-lived and used to cache the palette; it now re-reads
`iterm-colors.sh` on each state change, so drift is rare. When it still
happens:

- `iterm-restart-monitors.sh` — stop every running monitor and drop its
  pid file. A monitor restarts on the next SessionStart; a live session
  repaints on its next hook event.
- `iterm-reset-windows.sh` — clear our coloring back to the profile
  default on every TTY with a leftover state file (e.g. a session that
  exited without its SessionEnd hook firing).

### Why bash, not Python

Skill *helpers* are Python under `.claude/tools/` (see
[skill-tooling](skill-tooling.md)). These scripts are **not** skill
helpers — they are user shell-integration glue that runs from shell
hooks and an iTerm coprocess, where bash is the natural fit. So bash is
the deliberate call here, kept consistent by `shfmt` (format) and
`shellcheck` (lint), both wired into `cfg/pre-commit-lint.yml` scoped to
`.claude/scripts/`.

The one exception is `iterm-reorder.py`: tab reordering is only exposed
through iTerm2's **Python** API, so that file has to be Python. It is
linted by `ruff` like the rest of the repo's Python, and its ordering
logic is unit-tested under `make tools-tests` (the `.claude/scripts/`
discovery root).
