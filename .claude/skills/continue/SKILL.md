---
name: continue
description: Resume whatever was in progress after an accidental interruption — you hit escape, stopped the session, or usage ran out mid-task. Reconstruct the in-flight work from the conversation and task list and pick it back up as if nothing had happened, without re-asking what to do.
disable-model-invocation: true
user-invocable: true
---

# `/continue`

You were interrupted mid-task — an accidental escape, a stopped session,
or usage running out — and this session has resumed. Pick the work back
up **as if nothing had happened**. Do **not** re-ask what to build or
re-plan from scratch; the intent is already established in the
conversation.

## Steps

1. **Reconstruct what was in flight.** In priority order:

   - Check the **task list** (`TaskList`) for any task marked
     `in_progress` — that is what you were doing. If none is
     `in_progress`, the next `pending` task is what's next.
   - Re-read the **last user request** and your **last few actions** in
     the conversation to see where you stopped — a half-finished edit, a
     command you were about to run, a step you announced but didn't
     complete.
   - If a skill was running (e.g. `/init-pr`, `/review-pr`), resume it at
     the step you'd reached.

1. **Check for partial state before redoing anything.** An interrupted
   action may have half-landed. Before repeating a step, verify its
   current state so you don't double-apply it:

   - a file edit — Read the target to see if it already took;
   - a commit / push — `git status` and `git log` to see what's already
     recorded;
   - a command with side effects — check its effect before re-running.

1. **State briefly what you're resuming, then continue.** One line —
   "Resuming: \<the task>, at \<the step>" — then carry on to
   completion. Keep going through the remaining work the same way you
   would have if never interrupted.

1. **Only ask if genuinely ambiguous.** If the conversation truly leaves
   the next step under-determined (not just interrupted — actually
   unclear), make the best-supported inference and note the assumption,
   or ask a single focused question. Don't turn a resume into a
   re-specification.
