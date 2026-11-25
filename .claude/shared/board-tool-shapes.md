**Board-tool call shapes.** Copy these whole; never probe `--help`
or guess a flag:

```sh
python3 .claude/tools/board_batch.py state --id ENG-<n> \
  --state '<State>' [--dry-run]
python3 .claude/tools/board_batch.py edges --pairs <file> [--dry-run]
python3 .claude/tools/board_batch.py edges --blocker ENG-<a> \
  --blocked ENG-<b> [--remove] [--dry-run]
python3 .claude/tools/linear_issue.py read ENG-<n> --out <file>
python3 .claude/tools/linear_issue.py append --id ENG-<n> --file <file>
python3 .claude/tools/linear_issue.py create --body-dir <dir> \
  --state Backlog --milestone 'Session pending' [--parent ENG-<n>]
python3 .claude/tools/linear_patch.py patch --ops <file> ENG-<n>
python3 .claude/tools/session_dispatch.py [--wait <minutes>] \
  task <n> [+ chore <m> …]
```

A `--pairs` file is a JSON list of objects, each
`{"blocker": "ENG-<a>", "blocked": "ENG-<b>"}`. Each `--body-dir`
file opens with a `title: …` line, then optional `state:`,
`milestone:`, `priority:`, `verb:`, `substrate:` and `parent:`
lines, a blank line, and the body; one issue alone is
`--body-file <file> --title '<title>'`. Every write takes
`--dry-run`, and every flag precedes a dispatched verb.
