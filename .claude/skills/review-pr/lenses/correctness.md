# Lens: correctness

<!-- default-cap: ≤ 4 turns / ≤ 6 tool calls -->

**Your cap: `{{cap}}`, hard stop.**

## Dimension

Logic errors, off-by-ones, unhandled edge cases, incorrect assumptions,
broken invariants.

## Standing sub-question

**Mandatory on any diff touching dashboards, alerts, health tables,
status displays, or watchers: "if this were broken right now, would
anything actually emit?"** On an observability diff this is *the*
lens: rendering healthy while broken is a defect in the deliverable.
Ask it of the watcher and the alert, not only the panel.

## What you are handed

The diff, the excerpts and facts in your material, and any
file → symbol map the implement phase produced. On a concurrency- or
invariant-heavy diff that map is what spares you a re-survey: reason
from it rather than re-mapping the surrounding code.

When your scope line says you are the trivial tier's **one scoped
reviewer**, or the reduced tier's **value-rewiring** lens, it names
what else you cover (the new values and their call sites, say); the
dimension above still applies.
