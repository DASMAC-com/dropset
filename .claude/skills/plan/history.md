# `plan` history ledger

The measured incidents and retired designs behind rules in
[`SKILL.md`](SKILL.md), kept here so the entry file states each
rule once. Read on demand, never on invocation.

## Computing the session id (step 7)

One planning session ran a bare long-format listing of the
Claude projects folder to find its own transcript, at
**≈6.0k** for that single call — its sixth-largest result, and
≈6.4k across five such calls, making the listing that
session's top hardening candidate by result size. The next
bootstrap reproduced the same id by computation at near-zero
cost.

## Why the audit heartbeat writes no directive (step 6)

An audit is a real capacity spend, and the directive path
made it an invisible daily tax with three handoffs across two
skills and a document, plus a built-in staleness window
between writing the directive and firing it. The audits that
matter keep turning out to be issue-shaped — carrying scope,
rationale, and sequencing the way real work does — so they are
filed as issues and compete in the queue. The daily random
rotation this replaced had the same targeting failure from the
other direction: one pass filed **fifteen** parked findings,
several against maker-model and fair-value files that open
Backlog issues were already slated to rewrite. The engine was
working; the targeting was not — and because the parked pool
drains only through the promotion step (step 8, which runs at
bootstrap), over-filing costs the planning session directly.

## Why tiers, not structure, scope the bootstrap (step 1)

Filtering Todo to parents with **no children** was considered
and rejected — it inverts both real cases. The market-data
umbrella is a parent *with* children and the most important item
on the board; a childless issue may be the most parked thing
there is. The milestone is a deliberate signal; child count is
not.

## The meta fold (step 1)

**Why it is a default.** One bootstrap found **five** open meta
tasks — three of them unchained filings from automated passes —
and the fold had to be operator-prompted **twice** (three issues,
then all five). A later bootstrap found **seven** strays sitting
unblocked in the operator's Next view, which is what moved meta
filings onto the parking milestone.

**Why the pool includes unpulled batches.** The pool was once a
title scan of Backlog, which swallowed a prior unpulled batch as
a side effect. Narrowing it to the milestone lost that, and no
state check covers the gap — an unpulled batch sits in Backlog,
neither In Progress nor In Review — so two bootstraps with no
pull between would assemble two overlapping batches from the same
strays. The clause was first justified by the "exactly one meta
issue unblocked in Next" property; it survives that property's
retirement on the duplication argument alone.

**How serialization was retired.** The shape moved from a serial
chain with an edge per issue (08-18), to one edge behind the
batch (08-20), to one giant batch (09-02), to small edge-free
batches with no in-flight assembly gate (operator rule,
2026-09-11). The relation shapes went chain, then one edge, then
none — each removing relations rather than adding them, the right
direction for a mechanism whose risk is a spurious edge dropping
an issue out of the available set. The in-flight precondition
briefly outlived the edge it stood in for, making the retired
*wait for the one in flight* edge true by construction; with the
serialization itself abandoned, neither had anything left to
encode. The "exactly one meta issue in Next" property went with
them — deliberately, its job taken by the file-disjoint promotion
bound. Contention between parallel batches is paid as a rebase,
estimated (not measured) at order tens of cents in a short
session, against the quadratic a long one costs.

**Why 4–5 parts.** The batch size rests on the measurement and the
**\$155–198 versus \$455** modeling in
`docs/conventions/context-economy.md` → "Session length is itself
a cost lever". The same 2026-09-11 rule retired the `Claude:`
class's earlier no-size-bound exemption, so meta folds are bound
like any other filing.

**Ungated assembly and live specs.** With assembly ungated, the
never-fold-In-Progress
exclusion became load-bearing on its own: it is the only thing
between an assembly and a live spec. One batch of mined levers
landed as a new issue rather than on the prior batch for exactly
that reason — the prior one was live in a worktree.

**Why the zero-echo appender.** A ~30-line append to an open
`Claude:` batch cost **≈6.6k in one call**, that session's
largest result by a factor of **3.5** over the next, because the
batch had grown to ~6.6k of folded parts and a one-screen
addition paid for the whole history. An append has no anchor, so
leaving the MCP `patch` path loses no ambiguity protection.

**Vocabulary.** The operator's "Next" view was earlier called "Q".

**Why housekeeping no longer proposes meta merges.** Its last
board carve-out was retired once meta filings parked under the
milestone and this bootstrap swept it whole: a second writer on
that pool would only race the assembly.

## Bounding the Planning document (step 1)

The document's growth between rewrites is the same failure shape
as the retired Session Metrics inbox. The close-out rewrite bounds
a planning session's own appends,
but nothing bounded what accumulated between sessions:
`housekeeping` and implementation sessions appended post-close
notes with no consolidation owner, and an abrupt close skipped
the rewrite. One document reached the next bootstrap carrying
**two post-close sections plus a full day of in-session notes**.
The bootstrap-side rewrite was adopted at plan-20, which did the
consolidating rewrite mid-session on the operator's flag.

## Scope posture and re-scopes (step 4)

The scope posture lives here rather than in an implementation
session because scope is a staging decision and board analysis
is this session's monopoly; a separate skill would add a second
place that reasons about scope.

A re-scope appended as one paragraph once left a spec
contradicting itself: the prose said the scope was widening while
the then-current `**Touches**:` line still named a single file,
and the shipped diff spanned four areas. Retiring the field lost
the tell, not the failure.

## Coordinating with in-flight sessions (step 5)

In one planning session, eleven cross-session messages at ~0.5k
each prevented duplicated work three times; twice the message
*was* the amendment, saving both the body echo and a description
of the edit. A 09-11 form of the spec-fold rule was a three-part
act with a PR close in the middle; specs no longer sit on a PR
branch, so that act and its ordering hazard are gone.

## Cycling

The cycle design (command, ask-gated trigger, same-tab
relaunch, warm bootstrap) was ratified in an architect session
on 2026-10-05 to bound the hub's quadratic prefix growth. The
binding budget is the subscription window; the arithmetic that
sized the 250,000-token and 40-message thresholds used Bedrock
rates as its proxy, ratified as an assumption. Compaction was
rejected as the mechanism and stays a manual operator lever.
