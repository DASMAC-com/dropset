# Docs, prose, and spelling

## Docs and skills prose

**Refer to users in the abstract, never by name.** Committed docs and
skills (`.claude/skills/**`, `CLAUDE.md`, `docs/**`) should read as if
written for any user of the tool, so a particular individual's name
never appears in the prose — write "the user", "you", or "whoever runs
it" instead. The skill suite is general-purpose tooling; hard-coding
one person's name makes it read as bespoke and dates poorly. This is
about **prose only** — the env-var-resolved assignee / filing-destination
ids (`LINEAR_ASSIGNEE_ID`, etc.) are configuration, not prose, and are
unaffected.

## Line length

Docs and Markdown wrap at **80 columns** (MD013), and code at the
project's configured limit — both enforced by the `make lint` hook set
(the "Lines over 80 columns" hook for code, markdownlint for docs). So
**let the hook flag over-long lines** rather than pre-checking with a
manual `grep -nE '^.{81,}$'`: the manual grep just re-buys a result
`make lint` already produces, and its output lands in context (per
`docs/conventions/context-economy.md` → "Don't hand-run a check a hook
already owns").

## Spelling (cspell)

`cfg/dictionary.txt` is the **project-wide** spelling allow-list —
reserve it for terms that recur across the codebase. The rule: a word
belongs in `dictionary.txt` only if it appears in **≥ 2 files**. A term
used in just one file gets an inline escape in that file instead, by
comment style:

- Rust / TS / JS — `// cspell:word foo`
- Markdown — `<!-- cspell:word foo -->`
- YAML / TOML / shell — `# cspell:word foo`

The lone exception is a file that can't carry a comment (e.g.
`.json`), where the dictionary is the only option.

**Placement: one block at the top of the file, one word per line.**
All of a file's inline escapes go together in a single block at the
very top, never scattered beside each usage, and **each escaped word
gets its own directive on its own line** — never pack multiple words
into one comment. In **line-comment** files (Rust / TS / JS `//`, YAML
/ TOML / shell `#`) that's one directive per word on consecutive lines
with no blank lines between. In **Markdown** it's one
`<!-- cspell:word foo -->` per word, but mdformat inserts a blank line
between adjacent HTML comments, so the block is a blank-line-separated
stack of single-word comments — that's expected and stable, not drift.
"Top" means the first line, except where syntax forces something else
to lead: after a `---` YAML frontmatter block, after a `#!` shebang, or
after a leading module doc-comment / inner-attribute header. One known
place, one word per line, means a reader — and the audit — finds every
escape at a glance instead of hunting the file.

**Placing new words: use the helper, don't hand-loop cspell.** When a
diff introduces cspell-unknown words, don't iterate
`pre-commit run cspell` deciding dictionary-vs-inline by hand (that loop
has run ~20 round-trips and still slipped a word to CI). Ask
`.claude/tools/cspell_place.py`, which counts each word's spread across
the repo and prints the verdict per the ≥2-file rule — the dictionary
target, or the inline directive in the right comment style for the file:

```sh
python3 .claude/tools/cspell_place.py scan \
  --files path/to/changed.rs path/to/other.md
```

`scan` runs cspell to list the unknown words first (needs the cspell
CLI); `verdict WORD...` skips cspell and just places words you already
have.

The `cspell-audit` skill reconciles the dictionary against actual usage
**and** normalizes escape placement on this rule; run it when the
dictionary grows or escapes drift. `housekeeping` runs the same check
read-only and files any drift — a dictionary entry to move, or
mis-placed escapes to regroup — as a Backlog task.
