# Lens: `CLAUDE.md` and `docs/conventions/` freshness

<!-- default-cap: ≤ 4 turns / ≤ 6 tool calls -->

**Your cap: `{{cap}}`, hard stop.**

## Dimension

Does the convention set still match reality after this diff?
`CLAUDE.md` is the index; the rules live in `docs/conventions/**`.
Check the inlined sections' rules, command examples, paths and tooling
references against the codebase and the diff. Flag guidance the diff
outdates (a command, path, target or convention it renames, moves or
removes), any rule gone silently stale, **and any skill citing a
`CLAUDE.md` section or convention doc the diff renamed or moved** (the
index ↔ doc ↔ skill sync).

A rule the diff **directly violates or invalidates**, or a dangling
reference, is **blocking**; merely-stale prose is a **warning** with
the suggested correction. The main loop already ran the citation
check (`convention_refs.py`); its result is in your material, and
yours is the judgment half — whether the prose still describes the
code.

## Scope — a restriction, not a reading list

**Adjudicate from the inlined block; open a convention file only to
resolve a named dispute.** Your material carries at most two named
sections, excerpts inline.

## What you are handed

- The docs part of the diff plus a **claim → excerpt table**: the
  assertion, the 5–15 lines that settle it, a line-number column. The
  source part of the diff is **not** ground truth for a doc claim.
- Your named files (`CLAUDE.md`, the skill files, `test.yml`) as a
  **positive scope**, opened only through Read / Grep and slice-read —
  never shell `grep` / `git grep`, never a whole-file Read.
