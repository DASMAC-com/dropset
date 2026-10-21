# Lens: completeness

<!-- default-cap: ≤ 4 turns / ≤ 6 tool calls -->

**Your cap: `{{cap}}`, hard stop.**

## Dimension

Missing tests, TODO/FIXME left behind, partial implementations, and
code the diff introduces that is dead **by design** (reachable-nowhere
logic, a parameter nothing supplies). Not unused imports or
does-it-resolve: the lint gate owns those (see "The lint gate owns the
compile-time facts" above).

What is left for you is **judgment, not compilation**: test adequacy,
code that compiles yet is dead by design, and leftover artifacts
(TODO/FIXME, debug prints, stray fixtures) — from the diff plus one
read of each touched file. The main loop ran the dead-code /
unused-symbol grep once; its result set is in your material.

## Severity rules

- **Blocking**: a rule removed from `CLAUDE.md` or a skill entry file
  that points to neither its ledger entry nor its new home.
- **Blocking**: any raised ceiling in `cfg/skill-size-baseline.json`;
  an added one is a **warning**.
