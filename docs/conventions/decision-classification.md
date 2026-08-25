# Decision classification — what a skill may decide alone

Every skill in this repo draws a line somewhere between "decide it and
move on" and "stop and ask". Each one draws it in its own words, for its
own step, and the result is that the boundary is restated ad hoc a dozen
times and generalized nowhere. This doc is the shared vocabulary.

It is **descriptive of a rule we already hold**, not a new policy. The
strictest tier below already exists in exactly one place — the standing
prohibition on automated writers filing blocking edges — and it has held
up well. Lifting it into a named classification gives every other skill
the same three words to reason with.

## The three tiers

### Mechanical — decide silently

A decision with a defensible default and no meaningful alternative for a
human to weigh. Renaming a local variable while editing the function.
Choosing the loop shape. Wrapping a verbose runner. Picking the slice
bounds for a read.

Surfacing these is not caution, it is noise: a session that narrates
every mechanical choice buries the two that matter.

### Taste — decide, but surface at a final gate

A decision that has a real alternative a reasonable person might prefer,
where the cost of being wrong is bounded and reversible. The shape of a
new helper's interface. Which of two valid places a rule belongs in.
Whether to fold two sections or leave them apart.

**Decide it** — do not block on it — but say so at a gate the human is
already reading, and **name the alternative you did not take**. The
review summary and the `AskUserQuestion` handoffs are those gates. The
point is that the human can cheaply overturn it, not that they must
pre-approve it.

### User challenge — never decide

A decision that would **override something the operator explicitly
specified**. Not "something we infer they'd want": something they said.

A convention **counts as something they said** when it codifies a
standing operator direction — the blocking-edge rule below is written
as a convention precisely because it was ratified once and applies
continuously. What does not count is a convention's *implications*:
extending a rule by analogy to a case it does not name is inference,
and inference belongs in the tiers above.

Here the rule inverts. The operator's original direction is the
**default**, and the burden is on the argument for changing it. Fire
only when the case is strong enough to survive being argued against, put
it through `AskUserQuestion` with the concrete evidence, and take
silence as *no*.

## The canonical instance

**Blocking relations.** A filing skill that believes a real dependency
exists **proposes** it with evidence and writes it only on an explicit
yes; the default in any autonomous run is **no edge**. See
[linear-automation](linear-automation.md) → "Blocking relations".

The reasoning there generalizes, and is worth stating as the test for
which tier a decision belongs in: **the two error directions are not
symmetric**. A spurious edge drops an issue out of the operator's
available set and is expensive to notice; a missing edge costs a rebase.
When the asymmetry is that lopsided, the cheap error is the default and
the expensive one needs a yes.

So when classifying a decision, ask what each mistake costs:

- Both errors cheap and reversible → **Mechanical**.
- Errors bounded, but one direction is annoying to undo → **Taste**;
  decide, and surface it.
- One direction is expensive, hard to notice, or contradicts something
  the operator stated → **User challenge**.

## Two clarifications, because both have been got wrong

**"Propose, do not act" is the User-challenge tier, not a general
disposition.** Applying it everywhere produces a skill that asks
permission to do its job — which is why `audit`, `audit-scope` and the
review fan-out are explicit that **invoking them is the authorization**
for their sub-agent pass. Those are Mechanical: the fan-out *is* the
deliverable, and asking again is ceremony.

**A tier is about the decision, not about how destructive the action
is.** Destructive actions have their own separate protections — the
confirm-before-irreversible rule and the destructive-command guard hook.
A `rm -rf` is not "User challenge" because it is scary; it is gated
because it is irreversible. Keep the two axes apart, or every dangerous
Mechanical decision starts asking and every safe User-challenge one
stops.
