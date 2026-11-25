**Board-tool call shapes** — copy whole, never probe `--help`:

```sh
python3 .claude/tools/board_batch.py state --id ENG-<n> --state '<State>'
python3 .claude/tools/board_batch.py edges --pairs <file>
python3 .claude/tools/board_batch.py edges --blocker ENG-<a> \
  --blocked ENG-<b> [--remove]
python3 .claude/tools/linear_issue.py read ENG-<n> --out <file>
python3 .claude/tools/linear_issue.py append --id ENG-<n> --file <file>
python3 .claude/tools/linear_issue.py create --body-dir <dir> \
  --state Backlog --milestone 'Session pending' [--parent ENG-<n>]
python3 .claude/tools/linear_patch.py patch --ops <file> ENG-<n>
python3 .claude/tools/session_dispatch.py [--wait <min>] task <n>
```

`--pairs`: a JSON list of `{"blocker": "ENG-<a>", "blocked": …}`.
A `--body-dir` file is a `title:` line, optional `state:` /
`milestone:` / `priority:` / `verb:` / `substrate:` / `parent:`
lines, a blank line, the body. Every write takes `--dry-run`; every
flag precedes a dispatched verb.
