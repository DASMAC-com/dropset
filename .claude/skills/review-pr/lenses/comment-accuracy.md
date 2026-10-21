# Lens: comment accuracy (the prose-heavy pass)

<!-- default-cap: ≤ 4 turns / ≤ 6 tool calls -->

**Your cap: `{{cap}}`, hard stop.**

## Dimension

The diff is prose-heavy: a large share of its added lines are
comments. For each claim the new prose makes, **is it true of the code
in the same diff?** A comment that misstates the width, the rounding,
the order of operations, an invariant or a caller is a finding even
though it compiles.

## Scope — a restriction, not a reading list

**Adjudicate from the inlined block; open a file only to resolve a
named dispute.** Your material carries the source part of the diff's
comment hunks and the items they document — at most two named
sections, excerpts inline.
