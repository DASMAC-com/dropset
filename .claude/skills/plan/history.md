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

## Cycling

The cycle design (command, ask-gated trigger, same-tab
relaunch, warm bootstrap) was ratified in an architect session
on 2026-10-05 to bound the hub's quadratic prefix growth. The
binding budget is the subscription window; the arithmetic that
sized the 250,000-token and 40-message thresholds used Bedrock
rates as its proxy, ratified as an assumption. Compaction was
rejected as the mechanism and stays a manual operator lever.
