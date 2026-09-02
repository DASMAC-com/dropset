<!-- cspell:word empts -->

<!-- cspell:word EMPTS -->

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

**Never place a line wrap inside an inline code span.** `mdformat`
rejoins a line break that falls between backticks, and the rejoined
line is then long enough for markdownlint to fail it on MD013 — so the
two hooks fight, and the fix→lint loop does not converge on its own.
One session ran `make lint` **15 times (≈9.1k)**, four of them the same
failure, before the cause was identified. The autofix-vs-violation rule
in `review-pr` handles the *response*; this is the *cause*, and it is an
authoring rule: keep an inline code span entirely on one line, and wrap
before or after it. If that makes the line unavoidably long, shorten the
prose around the span rather than breaking the span.

## Rust intra-doc links

Intra-doc links are gated: the `rustdoc` hook in
`cfg/pre-commit-lint.yml` runs the workspace doc build with
`-D warnings`, so a broken, private, or redundantly-targeted link fails
`make lint` and CI. Two authoring rules follow, and both exist because
the failure they prevent is **remote** — it surfaces somewhere other
than the file you edited.

**A doc comment on an IDL-captured item takes a code span, never an
intra-doc link.** Codama copies the text of a doc comment on an
`#[account]` struct, a field of an instruction's `Accounts`, or any
IDL-carried type verbatim into `sdk/rs/src/generated/`, where a path
that resolves in the program crate resolves to nothing. The gate
enforces it, but it fails pointing at a generated file you never
edited — which is exactly why the rule needs stating rather than
discovering.

It applies **only** to items the IDL actually carries, and
`programs/dropset/src/state/market/layout.rs` holds both halves of the
distinction about 200 lines apart, so copy from there rather than from
the rule alone. `MarketHeader.head` is IDL-carried, so its doc writes
`` `NULL_SECTOR` ``. `Vault::next` is not — `Vault` is a bytemuck slab
struct the IDL never sees — so its doc writes
`` [`super::NULL_SECTOR`] ``, a working link. Prefer the link wherever
the item is not IDL-carried: a code span there is a loss, not a
safeguard.

**Never satisfy the gate by widening an item's visibility.** Making a
private item `pub` does make a public-docs link to it resolve, but
visibility is API surface in every crate — and in a consensus-critical
one it is a trust surface as well. Either way it is a real change made
to settle a docs lint. Degrade the link to a code span instead, or
re-path it (`[`Self::method`]` for a sibling method,
`[`super::CONST`]` for a parent-module const) when a real path exists.

The same holds one layer down, in the **generated** SDK: when a copied
link resolves there but hits a private generated module, widening
Codama's output is the identical trade and is equally not the answer.
The fix belongs in the program's doc comment, which is the one place
that owns the text.

One gap worth knowing: the gate does not pass
`--document-private-items`, so a broken link inside a **private**
module's `//!` block is not checked. Keep those correct by hand.

## `file:line` citations are derived LAST

When a change edits a source file **and** a doc that cites `file:line`
inside it, derive every citation **once, after all source edits have
landed** — never interleaved. Treat a citation written before a later
edit to the same file as presumed stale, and re-derive it.

**Do not arithmetic it forward.** Computing a new line number from hunk
offsets is a plausible-looking way to be wrong: one adversarial
cross-check did exactly that and got **255** where the real value was
**252**, caught only because it was then re-derived with a search.

This is filed for correctness rather than tokens — the token cost is
about 1k. One session derived its citations **three times**: after the
initial change (`context.rs:92`); after a review fix rewrote a doc
comment above that field, moving it to 94 and silently invalidating the
written citation; and after a cross-check nit added one more line,
moving it to 95 and the struct's closing brace from 169 to 170. Both
citations written in the second pass were wrong again. The branch had
already found pre-existing citations stale by 7 lines on one file and 3
on another, and its own spec instructed a future auditor to re-derive
rather than trust — so shipping a freshly stale citation in the commit
that fixes stale citations would have been self-refuting.

It is structurally likely to recur, which is why it is a rule and not a
note: `review-pr` applies fixes after the lens fan-out **and** again
after the cross-check, so any branch touching both code and a
line-citing doc gets at least two shift opportunities.

(A lint hook parsing `path:line` out of `docs/**` is worth considering
if this recurs; it is not proposed now — the prose rule is cheap, and
untested tooling here would need its own accuracy story.)

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

**cspell splits on hyphens and checks each part**, so a hyphenated
coinage is only as safe as its halves — `pre-empts` is checked as `pre`
plus `empts`, and fails on the second. Prefer an unhyphenated synonym
(`overrides`, `takes precedence over`) to adding a fragment to the
dictionary: a fragment is not a word, it would be a permanent entry
blessing a misspelling repo-wide, and the `--unique` sorter would keep
it alive.

The failure is invisible until a full lint round-trip spends itself on
it, which is why this is worth stating rather than discovering. One run
wrote "it pre-empts every median and pair row below" into a Rust doc
comment and the matching sentence in a doc; `make lint` then failed on
two unknown words that appear nowhere in the source — `empts` and
`EMPTS`. The split survives case, and neither reported word can be
found by searching for the word actually typed: a reader diagnosing the
failure searches for `empts` and finds nothing. The same split hits any
`re-`, `pre-`, `non-` or `co-` coinage, and this repo's prose carries a
lot of them.

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

**The dictionary union-merges — don't resolve its conflicts by hand.**
One sorted word per line means two branches that each add a word collide
at the same line by construction, and with several worktrees live that
was a recurring tax paid for a conflict with no semantic content. So
`.gitattributes` gives `cfg/dictionary.txt` git's built-in
`merge=union` driver: git keeps both sides' lines instead of raising a
conflict. Being built into git, it needs no `merge.*.driver`
configuration, so any clone resolves an add/add collision the same way.
Union merge neither sorts nor de-duplicates, so the merged file can be
out of order with a word twice. Both of those heal, because the
`file-contents-sorter` hook runs with `--unique`. Be precise about
**when**, though: this repo does not install pre-commit as a git hook, so
healing happens at the next **`make lint`** that includes the file — not
at commit time, and not in the merge commit itself. That is still
fail-closed rather than best-effort, and it is CI that closes it: the
`Lint` job runs the hook set over `--all-files`, so `cfg/dictionary.txt`
is in scope on **every** PR whether or not the PR touches it, and the
hook is a fixer — it exits non-zero when it rewrites. An out-of-order
dictionary therefore cannot survive a PR at all. A local `make lint` is
the path-scoped counterpart, which is why the healing commit is whichever
one happens to include the file. Nothing breaks in the interim either — cspell
tolerates both states.

One divergence is **not** self-healing, and `--unique` cannot help:
union merge keeps a line the other side **deleted**, and an edit is a
delete plus an add, so it also keeps both spellings of a reworded word.
That is a live shape rather than a hypothetical, because `cspell-audit`
drops a word that has fallen to a single file — so such a removal,
merged against a nearby addition, can come back. It is accepted rather
than fixed: a resurrected entry only over-permits one spelling, and the
next hygiene pass re-detects it. Don't expect the sorter to catch it.

**When you do resolve this file by hand, check completeness
mechanically** — the linters here verify *sortedness*, and sortedness
survives dropping a line, so a resolution that lost one side's word
lints perfectly clean:

```sh
python3 .claude/tools/merge_completeness.py --path cfg/dictionary.txt
```

It reports how many lines each side added, how many survived, and
itemizes every apparent loss so each one is explained rather than
assumed. This is the dictionary's sharpest need because `merge=union`
makes the *deletion* case silent in the other direction too.

Union merge is sound here **only** because the dictionary is an
unordered set that happens to be stored sorted. Do not extend the
attribute to a file where a dropped, doubled, or reordered line changes
meaning.
The same reasoning is why the Makefile declares each target's `.PHONY`
beside its own rule instead of in one central sorted block — a sorted
list is a merge-conflict generator, so prefer a layout that has no
single insertion point over resolving the collisions it creates.

The `cspell-audit` skill reconciles the dictionary against actual usage
**and** normalizes escape placement on this rule; run it when the
dictionary grows or escapes drift. `housekeeping` runs the same check
read-only and files any drift — a dictionary entry to move, or
mis-placed escapes to regroup — as a Backlog task.
