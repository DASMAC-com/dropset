Sessions of this kind deliberately run the most capable model —
**fidelity is the point**, and a session that has quietly landed on
the default implementation model will still *work*, which is exactly
why the slip goes unnoticed.

So on invocation, **before the bootstrap read**, check the model this
session is running as. The system prompt states it. If it is **not** a
Fable/Mythos-tier model, say so and offer the fix via
`AskUserQuestion`, recommended option first:

1. *"Run `/model fable` now and continue"* — recommended; it switches
   the running session in place.
1. *"Relaunch via `{{verb}}`"* — the deterministic path, at the cost of
   restarting the session.
1. *"Continue on this model anyway"* — proceed, and don't ask again
   this session.

This is the mirror of `init-pr`'s guard, pointing the other way: that
one catches a planning-tier model about to burn a long implementation
run, this one catches an implementation-tier model about to do work
that needs the top tier.

The `model:` frontmatter on this skill is **belt-and-braces, not the
mechanism**. Whether it switches the session going forward or applies
only to this invocation's execution is not specified, so it is not
relied on — `{{verb}}` passing `--model claude-fable-5` at launch is the
deterministic path, and the check above is what catches every other
route in.
