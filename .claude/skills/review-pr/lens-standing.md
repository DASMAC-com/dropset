## Scope of this review

Work **only** from the diff and commit log you were given. Dependency and
toolchain sources (`~/.cargo`, `node_modules`, another repo) are out of scope:
if you think you need a library's source to settle a point, say so in your
findings rather than going to look for it.

**Negative scope:** review the code diff only — do **not** audit permissions,
settings, or git history. Lenses have drifted into a permission-allowlist audit,
or run the full test suite instead of reviewing the diff, and the redo is
expensive. Stay on your dimension.

**Read your slice by path.** Your prompt names a diff slice in the scratchpad
(`review-diff-source.txt`, `-tests.txt`, or `-docs.txt`). Read that file; do not
re-derive the diff by shelling out to `git`.

## Budget

- Adjudicate from the provided diff plus the excerpts inlined in your prompt.
  **Cold-read only a file no excerpt covers.**
- Read every file you do need **once, up front**, and reason from that copy.
  Slice-read the large ones — Grep to the section, then `Read` with
  `offset`/`limit`. Re-reading a file on each turn pays for it on each turn.
- Your prompt states a cap in both turns and tool calls. It is a **hard stop**:
  at the cap, report what you have and flag anything unresolved. Do not
  continue past it.
- **Do NOT re-open a file a finding already cites** unless you are resolving a
  specific, named dispute about that exact file.

## Before you emit a finding

Every finding must name the diff line or the excerpt it rests on. If you cannot
point at the evidence in the material you were given, drop the finding rather
than reporting it as a suspicion — an unsupported finding costs more to
adjudicate than it can be worth.

State your confidence plainly, and prefer a short, specific finding to a long
speculative one. If your dimension turns up nothing, say so: a clean verdict is
a result, not a failure to look.

## Standing suppressions

Do not report any of the following — each is a deliberate, settled convention
of this repo, and re-litigating one wastes a round trip:

- The absence of AI attribution in commits or the PR body.
- Formatting that the lint hooks own (`rustfmt`, `ruff-format`, `biome`,
  `mdformat`, `taplo`). Lint runs separately and has already passed; a
  formatting nit here is noise. Flag a **lint-visible** problem only if you
  believe the hook itself is misconfigured, and say so in those terms.
- Missing `Co-Authored-By`, changelog entries, or version bumps — this repo
  uses none of them.
- `ENG-###` tags absent from the PR body: that is required, not an oversight.
