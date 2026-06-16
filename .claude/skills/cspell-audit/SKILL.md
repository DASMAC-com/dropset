---
name: cspell-audit
description: Enforce dictionary hygiene — every word in cfg/dictionary.txt must be used in at least two files; a word used in only one file is moved to an inline cspell escape in that file and dropped from the global dictionary, unless the sole file can't host a comment (e.g. JSON). Fixes directly when invoked; audit-loop runs the same check read-only and files linear-task issues.
disable-model-invocation: false
user-invocable: true
---

# `cspell-audit`

Keep `cfg/dictionary.txt` honest. The global dictionary
is a project-wide spelling allow-list, so a word earns a
place there only when it recurs across the codebase. A
word that appears in just **one** file isn't a
project-wide term — it's local jargon, and it belongs in
an **inline** cspell escape in that file, not the global
list. This skill audits the dictionary against actual
usage and reconciles the two.

## The policy

For each word `w` in `cfg/dictionary.txt`, count the
**distinct files** that contain `w` across everything
cspell scans (exclude `cfg/dictionary.txt` itself and the
config's `ignorePaths`):

- **≥ 2 files** → a genuinely shared term. **Keep** it in
  `dictionary.txt`.
- **exactly 1 file** → local to that file. **Remove** it
  from `dictionary.txt` and add an inline escape to that
  one file instead — *unless* the file's format can't
  host a comment (e.g. `.json`), in which case the global
  dictionary is the only place it can live, so **keep**
  it.
- **0 files** → dead entry. **Remove** it; nothing uses
  it anymore.

Matching is whole-word and case-insensitive (cspell
lowercases), so `Borsh` and `borsh` count as the same
word, and a word that is merely a substring of a longer
word does **not** count — match on word boundaries.

### Inline escape syntax by file type

Add the escape in a comment, following the repo's
existing `cspell:word` convention (see `CLAUDE.md` line
3). Place it at the top of the file, or adjacent to the
usage:

| File type           | Escape                                   |
| ------------------- | ---------------------------------------- |
| Rust / TS / JS      | `// cspell:word <w>`                     |
| Markdown            | `<!-- cspell:word <w> -->`               |
| YAML / TOML / shell | `# cspell:word <w>`                      |
| JSON                | *(no comment form — keep in dictionary)* |

Group several words for one file into a single directive
(`// cspell:word foo bar`).

## Input

Optional. With no argument, audit the **whole**
dictionary. Given a word or fragment, focus on the
matching entries (still report the rest).

## Steps

1. **Read the config and dictionary.** Read `cfg/cspell.yml`
   for its `ignorePaths`, and `cfg/dictionary.txt` for the
   word list — with the Read tool, never `jq`/`python`.

1. **Count usage per word.** For each dictionary word,
   find the distinct files that use it with one bare
   `git grep` per word — whole-word (`-w`),
   case-insensitive (`-i`), names only (`-l`), excluding
   the dictionary itself:

   ```sh
   git grep -ilw <word> -- ':!cfg/dictionary.txt'
   ```

   Then narrow to the files that **actually need the
   allow-list** — `git grep` over-counts:

   - Drop any hit under an `ignorePaths` glob.
   - Drop **generated / vendored** files: lock files
     (`Cargo.lock`, `*-lock.yaml`), `target/`,
     `node_modules/`, generated SDK / IDL trees. A
     hyphenated crate name in `Cargo.lock` can make
     `git grep` match one of its parts as a whole word,
     but cspell already accepts those (bundled
     dictionaries) and a regenerated file can't hold an
     escape anyway — so they must not inflate the count.

   The remaining count — hand-authored files where cspell
   would otherwise flag the word — is what the policy keys
   on. When unsure whether a file genuinely needs the
   word, the authoritative test is whether cspell flags it
   there with the word removed from the dictionary. Keep
   it one bare `git grep` per word so each call stays a
   `Bash(git grep:*)` allow-rule — don't pipe or chain.

1. **Classify each word** by the policy above: keep,
   move-inline, or remove-dead.

1. **Act by mode.**

   - **Direct run (default):** apply the reconciliation.
     For each *move-inline* word, Edit its sole file to add
     the format-appropriate `cspell:word` escape; for each
     *move-inline* or *dead* word, Edit `cfg/dictionary.txt`
     to drop the line (removing lines preserves the file's
     existing alphabetical order). If the plan is large,
     show it to the user before applying.
   - **Delegated run (`audit-loop`):** edit **nothing**.
     Return the violations — word, file count, the sole
     file, and the recommended action — so the loop files
     them via `linear-task`. See "Use from audit-loop".

1. **Verify.** Run the spell check to confirm the tree is
   still clean after reconciliation — an escape in the
   wrong comment form, or a word removed while still
   referenced, surfaces here:

   ```sh
   make lint
   ```

   Fix and re-run until clean.

1. **Report.** Tally: words kept, words moved inline (with
   their files), dead words removed, and any single-file
   words left in the dictionary because their file can't
   host a comment (JSON).

## Use from `audit-loop`

`audit-loop` runs this check **occasionally** — dictionary
drift is slow, so not every iteration. When it does, it
invokes `cspell-audit` in delegated mode (read-only, no
edits) and files each violation as its own Backlog issue
through the `linear-task` flow (env-resolved destination,
a `**Fingerprint**:` line so it dedups). Use
`dictionary:<word>` as the fingerprint — stable across
runs. This preserves `audit-loop`'s read-only guarantee:
the loop never edits source, it just files the finding for
a normal PR to fix.
