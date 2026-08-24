<!-- cspell:word gstack -->

<!-- cspell:word garrytan -->

<!-- cspell:word autoplan -->

<!-- cspell:word skillify -->

<!-- cspell:word devex -->

<!-- cspell:word wtree -->

<!-- cspell:word Greptile -->

<!-- cspell:word TTHW -->

# Surveying `gstack` against this repo's agent stack

A read-only survey of the public `garrytan/gstack` skill suite, run to
answer one question: what does it do that this repo's `.claude/` stack
does not, and which of those gaps are worth closing.

The survey ranks findings by their effect on the **cadence of the
self-improvement loop** — how fast a meta-work batch can be authored,
landed, and fed back in — rather than by raw capability. That weighting
is deliberate: the constraint on agent-infra work here is not ideas, it
is the round-trip time of a batch.

## Method, and what this survey is not

The upstream repo was cloned and surveyed with four scoped read-only
sub-agents, each given the canonical sub-agent brief and an explicit
path allowlist: the persona review skills, the execution-lifecycle
skills, the memory / safety / self-improvement skills, and the
skill-generation infrastructure. Browser internals, the iOS bridge, the
design-mockup pipeline, and the telemetry / hosted-knowledge stack were
out of scope throughout.

Two limits worth stating up front:

- **Claims were checked against source, not taken from the README.**
  One README-adjacent claim did not survive that check — see
  "Claims that did not verify" below. The rest of this document
  distinguishes what the code does from what the prose says.
- **No behavior was executed.** Nothing was installed, built, or run.
  Every mechanism below is read from committed source.

## What `gstack` is, structurally

Roughly thirty-five slash-command skills plus a set of standalone
binaries, arranged as a sprint: think, plan, build, review, test, ship,
reflect. Each stage writes an artifact the next stage reads. The
skills themselves are Markdown; the weight sits in three places that
are *not* Markdown, and those are the interesting parts:

1. A **code generator** that renders every `SKILL.md` from a `.tmpl`
   source with `{{PLACEHOLDER}}` substitution, gated in CI.
1. A set of **`PreToolUse` hooks** that enforce safety mechanically
   rather than by asking the model to be careful.
1. A **shared state bus** — append-only JSONL logs that skills write
   and later skills read.

## The persona pattern

This was the survey's primary target. The finding is that the personas
are a real mechanism, not a costume.

**They are different analyses, not different tones.** Each persona
reads different source material and is required to emit structurally
different artifacts. The CEO review produces an Error and Rescue
Registry; the design review generates actual rendered mockups and
scores them against named UX literature; the developer-experience
review greps README and CLI help text and produces
time-to-hello-world numbers against a competitive benchmark; the
security officer runs an attack-surface census with a
false-positive exclusion list. These are not one review relabelled
four times — they are artifacts a differently-skilled reviewer would
think to produce.

The convergence is at the edges. All four share a house style for
*expressing* expertise — a numbered list of named-authority heuristics
framed as "internalize these, do not enumerate them" — and several
individual heuristics appear near-verbatim in more than one persona.
So the technique is templated even where the analysis genuinely
differs.

**Modes are committed postures, not hints.** The CEO review has four
(Expansion, Selective Expansion, Hold Scope, Reduction); the
developer-experience review has three. A mode is selected by an
explicit user question with a context-derived recommendation — a
greenfield plan defaults to Expansion, a bugfix to Hold Scope, a diff
over fifteen files suggests Reduction — and once selected the
instruction is to commit fully and not silently drift.

**Composition is inline, not fan-out.** The orchestrator skill reads
the other skills' `SKILL.md` files from disk and executes their
instructions itself, in sequence, as one agent wearing four hats. It
does not dispatch four sub-agents. Sub-agents appear only for the
independent-challenge step inside each phase, where a second model is
asked to review the same plan with an explicit instruction not to read
the skill definitions — keeping the second opinion genuinely
independent.

Phase applicability is decided by **deterministic keyword counts**
over the plan text (two or more UI terms enables the design phase; two
or more API/CLI/SDK terms enables the developer-experience phase), not
by model judgement.

### The idea most worth stealing from the personas

A three-tier **decision classification** governing what the
orchestrator may decide alone:

- **Mechanical** — decided silently.
- **Taste** — decided, but surfaced at a final gate with the
  alternative shown.
- **User Challenge** — never decided. This tier fires only when two
  models *independently* recommend overriding something the user
  explicitly specified, and even then the user's original direction is
  the default that the models must argue against.

That third tier is the operational form of a principle the upstream
ethos states directly: models recommend, users decide, and agreement
between two models is signal rather than proof. This repo has the same
rule in exactly one place — the standing prohibition on automated
writers filing blocking edges, where a filer that believes a
dependency exists must propose it with evidence and default to no
edge. The rule is sound and it is not generalized.

## Where this repo already covers the ground

Confirmed by reading our skills, so that nothing here gets rebuilt:

| Upstream mechanism            | Our equivalent                        | Verdict |
| ----------------------------- | ------------------------------------- | ------- |
| Multi-lens adversarial review | `review-pr` lens fan-out              | Covered |
| Adversarial cross-check pass  | `review-pr` cross-check step          | Covered |
| Findings tied to evidence     | `review-pr` pre-emit gate             | Covered |
| Reflect stage                 | `session-metrics` plus `trim-context` | Covered |
| Cross-session memory          | The `memory/` store and its index     | Covered |
| Edit-boundary hook            | The worktree edit-path guard          | Covered |
| Dedup against resolved        | Audit and trim-lever fingerprints     | Covered |

Two of these deserve a note because the match is closer than expected.
Our `review-pr` already requires every finding to cite the exact
changed line it rests on and to drop any finding that cannot — the
same pre-emit discipline the upstream review applies. And our guard
hooks use the same `PreToolUse` mechanism, with the same fail-closed
reasoning, as the upstream safety rails.

The prior borrowing was real but is unrecorded: the string `gstack`
appears nowhere in this repo's prose, so there is no way to distinguish
"considered and declined" from "never examined". That is a small
recurring cost and is the reason this document exists as a committed
artifact rather than a session summary.

## Gaps, ranked by effect on loop cadence

### 1. Skill prose is hand-synced; upstream generates it

Our convention set states that changing a convention means updating
the `docs/conventions/` file **and** every skill that references it,
and that rule is enforced by a review lens and a housekeeping pass —
that is, by an agent remembering to look. Upstream renders every
`SKILL.md` from a template and enforces freshness as a build gate:
regenerate for all hosts, then fail on any diff against the committed
output, plus a second check that catches a newly-added skill whose
generated file was never committed.

This is the highest-cadence item in the survey because the
hand-sync tax is paid on **every** meta batch, and our largest skill
is over three thousand five hundred lines with substantial repeated
prose across siblings.

The transferable piece is not the whole generator. It is the narrower
one: extract the blocks that genuinely repeat across skills — the
sub-agent brief, the model guard, the context-economy rules — into
single sources, render them in, and gate freshness in CI.

### 2. Repeated cross-skill state has no zero-echo local bus

Upstream skills write append-only JSONL that later skills read: the
review log feeds the ship readiness check and the retrospective.

Our cross-skill state lives in Linear, deliberately, and the board
should stay there. But our own convention set already records that the
MCP write path echoes the whole stored body on every call, that this
is a fixed per-call cost which patching does not shrink, and that
fewer calls is therefore the only lever. The trim-lever writer exists
precisely as a documented zero-echo carve-out for an accumulator where
that cost compounds.

So the pattern is already validated here for one case. The gap is that
it was solved once, specifically, rather than generalized — and every
skill that accumulates structured findings pays the echo until it is.

### 3. Hardening candidates are identified but never acted on

Our tooling convention says that once a workflow is established and
repeated it should be hardened into a Python tool under `.claude/`,
and `session-metrics` already **produces the input signal** — it ranks
repeated command shapes worth hardening, labelled by cost class.

Nothing consumes that signal automatically. Upstream has a skill that
takes a proven ad-hoc flow and codifies it into a permanent,
self-contained, tested on-disk artifact, with a provenance guard, an
embedded fixture, a mandatory real assertion, atomic staging, an
approval gate, and post-commit verification.

Ours would be a different artifact — a stdlib Python tool with
`unittest` coverage and the skill reference wired in — but the
producer-to-consumer gap is the same, and closing it turns a
recurring manual step in the meta loop into a skill invocation.

### 4. Review-lens gating is by diff shape, never by yield

Our fan-out already gates lenses conditionally: security on a trust
surface, the two freshness lenses on touched paths, style skipped on a
meta-work diff. All of those gates key on **what the diff looks like**.

Upstream adds a second axis: a specialist that has returned zero
findings across ten or more dispatches is tagged a gating candidate
and skipped automatically, with named insurance specialists exempt
from ever being gated.

This matters here more than upstream because our own skill documents
the fan-out as the dominant cost of a review session — one measured
pass ran at roughly ninety-five percent of total session cost while
fully compliant with every prompt-tightening rule. Yield-based gating
attacks that cost from an angle prompt discipline cannot reach.

The exemption list is the essential half of the idea. A lens that
rarely fires is not the same as a lens that does not matter, and
security is the canonical example.

### 5. No debugging methodology exists in our stack

Upstream has a systematic root-cause skill built on two rules worth
quoting for their shape: an Iron Law that no fix may be written before
the root cause is investigated, and a three-strike rule that after
three failed hypotheses the architecture — not the hypothesis — is
what should be questioned.

At the limit it does not hard-stop; it forces an explicit choice
between continuing, escalating, and instrumenting-and-waiting. It also
scope-locks edits to the module under investigation for the duration.

We have nothing in this space, and the failure mode it prevents —
stacking speculative fixes without a confirmed cause — is not one our
review stage can catch, because by then the fixes are already written.

### 6. No destructive-command guard

Our three committed guards cover shell *form* (compounds, `git grep`)
and edit *path*. None covers command *danger*.

The upstream equivalent is a two-tier hook: an overridable ask for a
recursive delete, a destructive SQL statement, a force-push or a hard
reset; and an outright deny for a very small set of catastrophic
shapes — a recursive delete of root or home, and a force-push to the
default branch. It is honest about its own limits, describing itself
as a best-effort advisory stop rather than a policy boundary.

One implementation detail transfers directly as a warning. Its command
extractor was originally grep-based and silently truncated at escaped
quotes, letting a root delete through when it followed a quoted
argument; it now uses a real JSON parser. Our compound guard parses
the same tool input and is worth auditing for the same class of bug.

### 7. Verification freshness is re-run rather than content-bound

Our review skill re-checks base freshness before each expensive stage
and re-runs suites to establish currency. Upstream binds a claim to a
content fingerprint of the working tree — one that stays identical
through commits, rebases, amends and squashes — and grades recorded
evidence as fresh, stale or missing against it, so a passing result
can satisfy a later gate without re-running.

This is a cadence lever precisely because our current answer to "is
this still green?" is to pay for the suite again.

## The CEO question: a mode, and not on the skill it looks like

The framing to discard first: our `/plan` is not a weaker CEO persona.
Its eight steps are board bookkeeping — bootstrap the planning
document, keep the queue honest, curate blocking edges, file to
convention, coordinate sessions, write back, capture the token
profile, offer parked findings. Upstream's CEO review operates on a
single feature plan and challenges whether it is the right thing to
build at all. Same seat in the org chart; different job.

Taking as given that `/plan` already runs the top-tier model, owns the
board exclusively, and gates decisions through the question selector,
a persona adds three things beyond that, and only the first two are
worth having:

1. **A committed scope posture.** Our planning session has none — it
   is implicitly always holding scope, because a bookkeeper has no
   mandate to expand or cut. Naming the posture, defaulting it from
   context and requiring commitment to it is the substantive part of
   the upstream mode system.
1. **Required artifacts a bookkeeper would not produce** — the
   explicit enumeration of failure paths and of the gap between the
   current state and the intended one.
1. A distinct voice and heuristic list. This is the costume, and it
   is the part safe to skip.

**Recommendation: a scope-posture mode on `/plan`, applied per issue
at staging time — not a new skill, and not a gate in `init-pr`.**

The reasoning is a constraint, not a preference. "Is this the right
scope?" is a staging decision, and board analysis is the planning
session's monopoly here. Putting the challenge into `init-pr` would
relocate a board decision into an implementation session and break
that monopoly. Putting it in a separate skill would create a second
place that reasons about scope, which is the same violation with more
steps.

Two hard bounds this recommendation already respects. Planning
fidelity is never traded for tokens, so a design that shards personas
across cheaper models is out. And the proposal composes with the open
meta batch's reshaping of `/plan`'s bootstrap and close-out rather
than with the skill as committed.

## What not to adopt

- **Persona fan-out across cheaper models.** Pre-rejected: planning
  fidelity is not traded.
- **The review personas themselves.** Our lens fan-out plus
  cross-check already covers this; the survey's job here was to verify,
  not to rebuild.
- **Browser-driven QA, the design-mockup pipeline, the iOS bridge.**
  They solve problems this repo does not have, and they carry a
  browser daemon, an image API and a device bridge with them.
- **Telemetry, hosted knowledge base, cross-machine memory sync.**
  Out of scope and, for the memory piece, already served.
- **The full generator.** Its value here is the narrow extraction in
  gap 1, not thirty-five templated skills and a ten-host matrix.

## Claims that did not verify

The upstream ethos document states that its principles are "injected
into every workflow skill's preamble automatically". Reading the
generator, the only path that touches that document emits a condensed
summary and a **link** to it, and does so only for skills at preamble
tier three or higher. The full text is never inlined anywhere, and
lower-tier skills get nothing.

The mechanism is real and the claim overstates it. Recording this is
the point: the same reading discipline should apply to the rest of the
upstream prose, some of which is marketing.

One related correction to a first impression. The model overlays are
resolved at **generation** time from a build flag and baked into the
committed skill file — they never detect the running model. They are
therefore not a replacement for our runtime model guard, which covers
a case they cannot. What they add is per-model behavioral
calibration, with a cycle-guarded inheritance directive, and every
overlay is explicitly subordinate to the skill's own workflow, stop
points and question gates.
