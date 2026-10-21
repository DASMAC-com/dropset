# Lens: move fidelity and straggler references

<!-- default-cap: ≤ 4 turns / ≤ 6 tool calls -->

**Your cap: `{{cap}}`, hard stop.**

## Dimension

You are the reduced tier's one lens for a **faithful extraction or
move refactor**: code deleted in one place reappears verbatim
elsewhere. Two questions:

1. Is the move faithful — no dropped or altered lines?
1. Does every reference reach the new home?

Do not review the moved, unchanged logic for correctness, completeness
or style; it was reviewed where it lived.

On a **mechanical repo-wide reformat** your scope line says so, and
the question narrows to one: did any semantic change ride along with
the reformat noise?

## What you are handed

The straggler hit-list from the main loop's Grep-tool sweep. Adjudicate
from it. If you must search again, use the **Grep tool**: it honors
gitignore, while a bare recursive shell `grep` over a package root
walks build output.
