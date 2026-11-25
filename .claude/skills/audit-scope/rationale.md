<!-- cspell:word Gstack -->

# `audit-scope` rationale

The why behind the rules in [`SKILL.md`](SKILL.md): measured
incidents, retired designs and edge-case reasoning. The skill
states each rule; read this file when a rule's edge is in
question, never as part of the step list.

## Origin: parked issues, not a checklist

This skill replaces the old `audit-codebase`, which wrote a
gitignored checklist. Findings now live as real Linear issues —
so they dedup and search like any other — but **parked**, so
filing them does not put work in the pull queue.

`review-pr` handles its fan-out the same way: invoking it
pre-approves its regular fan-out; its one entry question exists
only to offer an abridged tier on a small diff, and an
unanswered one proceeds at the regular tier.

## The audit issue's state

Marking the audit issue Done at the handoff is the measured
mistake that produced the In Progress / In Review rule. Under
the no-PR shape the issue state is the only signal that the work
is live, so an honest state is the whole of this skill's
obligation. The transcript is *not* covered by the issue's
state: the conversation purge protects only a transcript on an
open PR, which an audit never has.

## Lane tiers

Mechanical lanes on the helper tier are not the standing
rejection of Sonnet-tier task workers (2026-09-11), which covers
implementers: a read-only lane whose error mode is a false
positive the skeptic kills downstream is not one
(operator-confirmed, 2026-10-22).

## Splitting dimensions at plan time

The sub-agent brief tells every agent not to spawn sub-agents of
its own, but a dimension whose scope spans obviously separable
lanes invites exactly that: comment-accuracy over source
comments *plus* a spec doc *plus* five READMEs is three jobs
wearing one name. One such agent fanned out four grandchildren
for ≈8.8M of per-turn input that no skill asked for, and the
fan-out was invisible to the rotation's own plan.

The comment-accuracy budget (about 25–30 non-test source files
per lane) exists because two unbudgeted lanes once cost ≈ \$12
of one audit's fan-out, at 73 and 44 turns.

## The self-deflation clause

Measured: five dimension agents returned **46** findings and two
skeptics killed **37**, an ~80% false-positive rate upstream of
the skeptic, which earns its cost. The clause is the cheapest
noise gate in the skill.

The skeptic's sideways sweep and evidence-state duties, and the
coverage line, are borrowed from Gstack's review and cso skills.

## A quoted migration

An applied migration is immutable — the runner hashes the raw
bytes and rejects any change, so even a comment cannot be
reworded — which makes it the **least** authoritative statement
of current behavior while looking like the most authoritative
one. Measured instances: an asymmetry claim in migrations 0003
and 0008 was copied into two prose docs and both went stale
silently when the world it described was corrected, and
migration 0009 states a false invariant (that a source writes
bars or ticks and never both, untrue for the coinbase source)
whose correction lives in 0010's text instead. Anything
reasoning off a migration's stated invariant may be reasoning
off a false premise that can never be fixed in place.

## Instruction integrity: verify against the parser

A real instance: the sub-agent brief named `--count` / `--files`
on `search_source.py`, which has neither. A docstring and an
`add_argument` call drift independently, so a prescribed command
is checked against the argument parser, never the tool's prose.

## Why not `save_issue`

The MCP echoes the entire stored body back on a *create* as
well, which makes it worst for precisely this caller: an audit
files folded issues with a part and a fingerprint per finding,
so the bodies are long **by design** and the cost grows with
exactly the thing the fold rule encourages. Measured (session
fa6fc519, the seam audit): `save_issue` was the top main-loop
sink at **9 calls / ≈22.8k**, supplying six of the session's
eight largest single results, while the entire rest of the main
loop's tool results came to roughly 23k combined — one shape,
about half the cost. Every echoed byte was content the session
had just authored, so nothing decision-relevant is lost.

State, milestone and priority are writer *parameters*, which is what lets one
filer serve this skill's Backlog + `Session pending` staging and the scheduler's
plain Backlog follow-up filings (27 calls / ≈30.1k in one measured pass) without
a second implementation.

## No edges, no collision links, no `Touches`

Filing here is **autonomous**: there is nobody to answer an
`AskUserQuestion` mid-rotation, and the ratification review asks
about findings, never edges, so the default is no edge. A
spurious edge drops an issue out of the board's available set; a
missing one costs a rebase. Coupling that means *one PR* is the
merged-issue case, and is unaffected.

The automated file-collision machinery is **retired**: there is
no per-issue sweep, nothing to build around one, and nothing to
reintroduce. Collision reconciliation belongs to a scheduler
session, judged on content from its own board read. The
declared-scope `**Touches**:` glob field is retired with it
(`CLAUDE.md` → "Structured filing fields"): nothing consumed it
once the collision machinery went.

## The fingerprint is the stable key

Every citation and quoted snippet in a finding is a snapshot of
the commit it was *discovered* at, and an issue may sit in the
Backlog for months while unrelated PRs rewrite around it. One
session's surfaced task named four line-anchored citations
across two files and **all four** had been rewritten away, with
one named file carrying no relevant references at all — the work
item was moot, at the cost of four exploratory greps to
establish it. The implementer re-derives the location; the
fingerprint is what survives.

## Coherence floor against the size bound

A coherent set larger than a short session splits into
sequential PRs because session cost is roughly quadratic in
session length (`docs/conventions/context-economy.md` →
"Session length is itself a cost lever"). Where the two pull
against each other, coherence wins at the floor and the size
bound wins at the ceiling. Full rule: `CLAUDE.md` → "Structured
filing fields" / `docs/conventions/linear-automation.md` → "Fold
coupled findings into one issue".
