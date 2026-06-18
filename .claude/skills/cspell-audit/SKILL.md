---
name: cspell-audit
description: Enforce spelling-escape hygiene — every word in cfg/dictionary.txt must be used in at least two files (a word used in only one file is moved to an inline cspell escape in that file and dropped from the global dictionary, unless the sole file can't host a comment, e.g. JSON), and every file's inline escapes sit in one contiguous block at the top. Fixes directly when invoked; housekeeping runs the same check read-only and files linear-task issues.
disable-model-invocation: false
user-invocable: true
---

# `cspell-audit`

Keep `cfg/dictionary.txt` honest **and** keep inline
escapes tidy. The skill enforces two rules:

1. **Dictionary membership.** The global dictionary is a
   project-wide spelling allow-list, so a word earns a
   place there only when it recurs across the codebase. A
   word that appears in just **one** file isn't a
   project-wide term — it's local jargon, and it belongs
   in an **inline** cspell escape in that file, not the
   global list. This skill audits the dictionary against
   actual usage and reconciles the two.
1. **Escape placement.** Every file's inline
   `cspell:word` escapes must sit in **one contiguous
   block at the top** of the file — not scattered beside
   each usage, not split by blank lines. The skill
   normalizes any file whose escapes have drifted from
   that shape.

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
`cspell:word` convention (see `CLAUDE.md` → "Spelling
(cspell)"):

| File type           | Escape                                   |
| ------------------- | ---------------------------------------- |
| Rust / TS / JS      | `// cspell:word <w>`                     |
| Markdown            | `<!-- cspell:word <w> -->`               |
| YAML / TOML / shell | `# cspell:word <w>`                      |
| JSON                | *(no comment form — keep in dictionary)* |

### Placement: one block at the top

A file's escapes all live together in a **single block at
the very top**. The exact shape depends on the comment
style:

- **Line-comment files** (Rust / TS / JS `//`,
  YAML / TOML / shell `#`): one directive per word, one
  word per line, consecutive lines with no blank lines
  between. Don't pack several words onto one line — one
  per line keeps the block diff-friendly:

  ```txt
  // cspell:word luhansk
  // cspell:word noninteractive
  ```

- **Markdown** (`<!-- … -->`): mdformat inserts a blank
  line between adjacent HTML comments, which would break a
  multi-line block, so use a **single** comment with every
  word space-separated on one line — that one comment
  *is* the block:

  ```txt
  <!-- cspell:word oneline unstarted -->
  ```

"Top" means the first line, except where syntax forces
something else to lead — put the block immediately
**after**: a `---` YAML frontmatter block (Markdown skill
files, workflow YAML), a `#!` shebang (shell scripts), or
a leading module doc-comment / inner-attribute header
(Rust).

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

1. **Scan escape placement.** Independent of the
   dictionary, audit the inline escapes already in the
   tree. Find every file that carries one with a single
   bare `git grep` (names only, the literal directive):

   ```sh
   git grep -l "cspell:word" -- ':!cfg/dictionary.txt'
   ```

   Read each hit and flag it as **mis-placed** when its
   escapes aren't already the single top block its comment
   style calls for (per "Placement" above): escapes
   scattered beside their usages, a line-comment block
   split by blank lines or packing several words on one
   line, a Markdown file using several `<!-- … -->`
   comments instead of one combined comment, or the block
   sitting below the first position syntax allows
   (frontmatter / shebang / module header). A file whose
   escapes are already a clean top block is fine — leave
   it untouched.

1. **Act by mode.**

   - **Direct run (default):** apply both reconciliations.
     For each *move-inline* word, Edit its sole file to add
     the format-appropriate `cspell:word` escape **into
     that file's top block** (create the block if absent,
     append a line if it exists); for each *move-inline* or
     *dead* word, Edit `cfg/dictionary.txt` to drop the
     line (removing lines preserves the file's existing
     alphabetical order). For each **mis-placed** file,
     Edit it to gather its escapes into one top block in
     the file's comment style (one word per line for
     line-comment files; a single combined `<!-- … -->`
     for Markdown), removing the old scattered copies. If
     the plan is large, show it to the
     user before applying.
   - **Delegated run (`housekeeping`):** edit **nothing**.
     Return the violations — for a dictionary word: the
     word, file count, sole file, and recommended action;
     for a mis-placed file: its path and that its escapes
     need regrouping — so the caller files them via
     `linear-task`. See "Use from housekeeping".

1. **Verify.** Run the spell check to confirm the tree is
   still clean after reconciliation — an escape in the
   wrong comment form, a word removed while still
   referenced, or a directive a normalization edit dropped
   by mistake surfaces here:

   ```sh
   make lint
   ```

   Fix and re-run until clean.

1. **Report.** Tally: words kept, words moved inline (with
   their files), dead words removed, any single-file words
   left in the dictionary because their file can't host a
   comment (JSON), and files whose escapes were regrouped
   into a top block.

## Use from `housekeeping`

The `housekeeping` skill runs this check on its periodic
pass — escape drift is slow, so it's upkeep, not part of
the audit loop. It invokes `cspell-audit` in delegated
mode (read-only, no edits) and files each violation as its
own Backlog issue through the `linear-task` flow
(env-resolved destination, a `**Fingerprint**:` line so it
dedups). The fingerprint is stable across runs and keyed
by the violation kind:

- dictionary drift → `dictionary:<word>`
- mis-placed escapes → `cspell-placement:<path>`

This keeps the housekeeping pass non-editing: it never
touches source, it just files the finding — a word to move
or a file's escapes to regroup — for a normal PR to fix.
