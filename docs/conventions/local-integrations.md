<!-- cspell:word zshrc -->

<!-- cspell:word Prefs -->

# Local integrations

This doc covers the **user-local Claude Code configuration** the repo
*documents but does not commit*: the compound-shell guard hook, the
iTerm2 tab-color integration, and the shell (`~/.zshrc`) setup they lean
on. None of it is enforced on a checkout.

Both `.claude/settings.json` (hook + permission wiring) and
`.claude/settings.local.json` (the per-machine allowlist) are
git-ignored — the repo *documents* how to configure your own Claude
Code, it does not push hooks or permissions onto a contributor or a CI
runner (see [commits and PRs](commits-and-prs.md) for why settings stay
out of the tree). So everything here is **opt-in**: a fresh worktree or
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

`AskUserQuestion` does not fire a `Notification`, so the tool's green is
never overwritten by a stray permission yellow.

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
  whose action is **Run Coprocess…**, pointing at
  `.claude/scripts/iterm-attend.sh`. (A coprocess inherits
  `$ITERM_SESSION_ID`, which is why the attend script can resolve its tty
  from the registration above.)
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
