# Lens: CI skip-list freshness

<!-- default-cap: ≤ 4 turns / ≤ 6 tool calls -->

**Your cap: `{{cap}}`, hard stop.**

## Dimension

The `Tests` workflow's `changes` job is a fail-**closed** `code`
filter: a catch-all `'**'` minus **negated** excludes
(`'!frontend/**'`, `'!docs/**'`, …) under
`predicate-quantifier: 'every'`, so an unlisted new path runs the
suite automatically. This lens is about the opposite: a diff that
**adds or renames a test-IRRELEVANT tree** (a frontend-like dir, a
TS-only SDK package, a docs or config tree, a non-test workflow)
leaves the exclude-list stale.

Grep `.github/workflows/test.yml` to the `code:` /
`predicate-quantifier` block and slice-read just that (~20 lines),
then flag the one-line `'!tree/**'` addition or rename. Always a
**warning**: a stale list only over-runs tests.

## What you are handed

Your named files as a **positive scope**, opened only through Read /
Grep and slice-read — never shell `grep` / `git grep`, never a
whole-file Read.
