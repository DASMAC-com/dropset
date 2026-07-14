<!-- cspell:word zshrc -->

<!-- cspell:word reorderer -->

<!-- cspell:word venv -->

<!-- cspell:word repoint -->

# Local integrations

This doc covers the **user-local Claude Code configuration** the repo
*documents but does not commit*: the compound-shell guard hook, the
worktree edit-path guard hook, the iTerm2 tab-color integration, and the
shell (`~/.zshrc`) setup they lean on. None of it is enforced on a
checkout.

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
`firm-perms` skill maintains the local allowlist for you. Because
neither settings file is tracked, a worktree does **not** inherit the
base repo's copy automatically; `firm-perms`' full sweep is what
propagates a firmed allowlist from the base repo into a worktree (and
back), so run it once in a cold worktree if the guard or a familiar
allow-rule is missing.

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
  mark-as-unread.
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

- **Linear MCP ids** — `LINEAR_TEAM_ID`, `LINEAR_PROJECT_ID`,
  `LINEAR_ASSIGNEE_ID`, and `LINEAR_API_KEY` (see
  [linear-automation](linear-automation.md) for what reads them). The
  API key is a secret; keep real values out of any committed file.

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
