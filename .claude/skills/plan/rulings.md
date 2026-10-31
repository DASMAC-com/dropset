<!-- cspell:word antichain -->

<!-- cspell:word GBPA -->

# `plan` rulings and doctrine

The operator's standing rulings, the fleet doctrine and the
standing decisions a scheduler session obeys. They are
**instruction, not state**, so they live here, committed and
reviewed, rather than in the Planning document. The bootstrap
reads this file **before** the document (`SKILL.md` step 1); the
entry file states the method and is never a copy of this.

The numbering is stable — issues and messages cite "ruling 7" —
so a retired ruling keeps its number with a strike note rather
than being renumbered away. Live instances of a ruling (which
issues a freeze names, which chain a split re-pointed) are
**state**, and live in the document; a ruling here states only
the rule.

**Adding or amending a ruling is a reviewed change.** Planning
files a `Claude:` issue carrying the ruling text, which lands as
a PR to this file; until it lands, the ruling sits under the
document's `In-session notes` heading, and each close-out
carries it forward there until it merges. Never park a ruling in
the document's state sections.

## The numbered rulings

1. **Tier names are advisor / executor / helper / background.**
   Advisor: `architect` and `explore anthropic` on the Anthropic
   subscription, `plan` on Bedrock, `explore` on Bedrock with
   executor-tier sub-agents. Executor: task workers on Bedrock, and
   the launcher (`launch`), which runs the lever fold in its own
   tab (`housekeeping`, once the seat exception here, is retired).
   Helper: `chore`
   sessions, Sonnet-class, Bedrock by default, pinned with the 1M
   suffix in the runtime config.
   Model ids live only in the operator's runtime config; unset
   refuses the launch. Role nouns (2026-10-26): the **scheduler**
   (`plan`), the **curator** (`curate`) and the **launcher**
   (`launch`).

1. **Next means dispatchable right now.** A planning hold is a
   blocking edge the operator can see, never prose-only state.
   Automated filers never place an edge; the human sources are a
   planning session, a seat session's filing yes in the operator's
   tab (`ratify-staged`), and an edge the operator ticks in the
   curator's tab (ruling 12). (`SKILL.md` step 2.)
   **The coordination rule** (2026-10-26, amended 2026-10-28 and
   2026-10-29): the curator judges — promote, kill, merge; the
   scheduler schedules, owns the direction and the Planning
   document; the launcher starts the day and hands the curator its
   kill and merge candidates. **Both advisors hold priority
   authority** (2026-10-29), in Backlog and in Todo alike, under
   the **agreement rule**: a priority write is messaged to the
   other session in the same turn with the reason; the other may
   answer with a counter-write and its reason; a third exchange on
   one issue goes to the operator through `AskUserQuestion` in the
   tab that last wrote. The scheduler owns dispatch order as read:
   it dispatches on the priorities as they stand at the moment of
   the wave. **Every filer proposes**: the filing ask carries a
   priority per task, the yes writes it in the creating call, and
   the filer messages the live scheduler and curator the set in one
   line; either confirms by silence or rewrites under the agreement
   rule, and with neither live the proposal stands and is named in
   the handoff. `[Operator]` items stay fixed at Urgent. (Struck
   2026-10-29: the 2026-10-28 clause "priorities are the
   scheduler's alone", under which the curator only messaged a
   priority it believed wrong and waited.) Both write edges
   anywhere; an edge write touching what the other placed is
   messaged to it in the same turn with the reason; neither
   deletes the other's edge without a message; the document
   records the result at the scheduler's next write-back.
   **The close** (2026-10-28): on the operator's
   close-out yes the scheduler messages a live curator "closing
   for the day", waits for no answer, and reports
   `curator: not live` otherwise; the curator finishes a batch
   mid-tick, takes no new one, states the inbox remaining, runs
   its metrics tail and prints `CLEAR TO CLOSE — close this window`.

1. **The necessity kill gate.** A pool member
   that cannot name a concrete recurring cost larger than its
   landing-plus-ratification cost is closed, and doubt still
   kills. Doubt means doubt the issue is worth doing, never "not
   this week". Relocated 2026-10-26: the curator's commentary
   recommends cancel with the reason; the operator's drop comment
   executes it. The one-time cull of the whole meta stock ran on
   2026-10-15 (34 closed, 12 folds).

1. **Headroom ratchet.** A shrinking PR commits its ceiling at
   size times 1.10; bytes-per-token measured at 2.76 to 2.78.

1. **Queue watch.** A review session never hands the merge wait
   to the operator.

1. **AWS.** Every suggested login lets the operator pick the
   identity: the login gate runs
   `aws sso login --use-device-code --no-browser` against an SSO
   profile, and refuses a non-SSO one with the `aws configure sso`
   instruction. The spend cap
   is live at 2,000 dollars per rolling 24 hours;
   **on a sudden Bedrock access-denied, check the kill switch
   before credentials.** Key rotation is yearly.

1. **Mid-flight splits: one task per PR.** A worker that finds
   its issue too large finishes one self-contained PR, messages
   planning the remainder as pullable units, and ends its
   session; planning files them and **re-points the blocking
   edges to the chain's tail**, so ratifying the first issue does
   not release work that depends on the whole, and messages the
   curator the filing (2026-10-26). (`SKILL.md` step 5.)

1. **Meta stack deliberateness.** Meta work is dispatched
   deliberately, not as a constant stream.

1. **Atomic session close.** Mark Done first, remove the
   worktree second; the close tool is the session's last call.

1. **AWS operator chores are one attended session.** A program
   probe (the loader-v4 probe) stays a separate program item.

1. **Fan-out gate.** Invoking a verb pre-approves its
   regular sub-agent fan-out — `review-pr`'s reviewers,
   `audit-scope`'s cross-check. Invoking `launch` likewise
   authorizes its lever fold's Linear writes.

1. **Architect read-back: two stages, two questions.** Stage 1 is
   the spec: fold the operator's comments into it in place; it
   lives at the worktree root, never under a repo-shaped path, is
   expendable once the rulings are on the issue, and ends with the
   task list. Nothing is filed in stage 1. Stage 2 is the tasks,
   through `ratify-staged` (2026-10-23): the **filing ask** names
   the N tasks and the proposed edges, and its yes files them
   `Backlog` plus `Session pending` and places the edges it did
   not strike; the **review ask** is one checkbox per task, a tick
   with no comment approving it, repeated over the held and
   unticked until none is held. The carve-out: on that final
   answer the session moves the whole set to Todo in one batch —
   never on its own judgment — and its close gate asserts none is
   still parked. The same machinery serves explore sessions and
   audit findings. **The
   curator's carve-out** (2026-10-26): it moves a ticked,
   uncommented pool member to Todo one at a time, the tick in its
   tab being the operator's move, and executes an edge, kill or
   merge the operator ticks in the same tab, and writes priorities
   under ruling 2's agreement rule — messaging the scheduler each
   one.

1. **Planning is read-only toward the repo.** A scheduler session
   writes Linear state only; a fast-forward of the base checkout
   is not a planning edit. Edits to the operator's runtime config
   on the operator's word are outside the repo and allowed, and so
   are memory notes, which live outside it.

1. **The deploy ceremony is code.** Committed commands with
   deterministic 1Password secret references; the agent never
   holds a secret. The program keypair is stored as a
   128-character hex string of the 64-byte keypair; the cockpit
   decodes hex, base58 or JSON in memory.

1. **Session metrics is a digest, not a session step.** A session
   prints a tool estimate at its end, writes its account, and files
   nothing; upkeep digests every transcript each morning and the
   launcher's lever fold files from the digest. The cost line on
   the issue is the close tool's write.

1. **Housekeeping never costs an extra tab visit.** Upkeep runs
   as the pre-launch in the typed tab, its report in the
   scheduler's bootstrap; the lever fold runs in the launcher's
   tab, and destructive items are asked once there, in one ask,
   applied only on a yes (2026-10-28).

1. **Dispatch never steals focus, and no notification for
   self-waiting.** A yellow tab is a trigger problem, not a window
   problem: it turns yellow only for a request actually waiting on
   the operator, never for classifier-approved requests in rapid
   succession. The debounce is an environment variable with a
   5-second default.

1. **No hardcoded 200ms.** Expiry defaults derive from the
   observed slot pace.

1. ~~**Meta freeze until C2.**~~ Retired 2026-10-23: a parking
   milestone means "not yet reviewed or assembled", never "not
   now", so nothing sits in a giant backlog (ruling 3).

1. **Parked hygiene is a rhythm.** Every full bootstrap says each
   parked milestone's count with its direction, machine-written by
   upkeep; the digest's levers fold there (ruling 23), Audit
   findings drains by promotion or kill, and Session pending gets
   a count-and-offer line (`SKILL.md` step 8). Follow-ups and
   Claude meta left the rhythm 2026-10-26: filers no longer park.

1. **Alpenglow proof is explicit.** The v3 port closes only with
   a recorded end-to-end deploy on an Alpenglow-enabled 4.3
   localnet; C2 is the real-cluster proof; a pre-C2 devnet
   rehearsal is asked, not assumed.

1. **The tGBP basis is observed, not pinned.**

1. **Trim tasks must not accumulate.** Sessions report levers in
   their account and file none; the launcher's lever fold
   files from the digest every morning, folding a near-duplicate
   into the open Backlog issue rather than filing a sibling, and
   the curator recommends cancel on doubt (ruling 3). A Backlog of similar trim
   tasks is an analyst defect (raise the reservoir bar), not a
   backlog. A couple hundred dollars a day is acceptable spend;
   unbounded growth is the only failure watched.

1. **Chores are separate issues from tasks.** A chore worktree is
   the bare issue number; minor non-repo work (key grinding, local
   deploys, probes) dispatches as `chore <n>`, never `task <n>`;
   `chore anthropic <n>` for web research.

1. **No placeholder at the localnet program id.** The localnet
   secret is public, so occupying that address on mainnet invites
   a land-grab race planning will not enter. The SDK alone
   defends: the cluster or program id is a required argument on
   every client path, and the SDK refuses a non-localnet cluster
   with the localnet id.

1. **The first mainnet publish carries admin-teardown** — the
   documented early-mainnet build. The upgrade authority is
   retained after the publish, probably in perpetuity (operator,
   2026-10-30, on 1923); admin-teardown may be removed by a later
   upgrade on its own, and no fully immutable deploy is assumed or
   scheduled. A later upgrade is an additive change under that
   authority, never a republish.

1. **Operator-executed items are never dispatched.** An item
   executed in a vendor dashboard, credentials or deploy env is
   asked through `AskUserQuestion` in the tab that needs it, or
   filed at Urgent, Backlog, no milestone, surfaced once in review
   by the curator: as an `[Operator]` task, which lives in Todo,
   carried one per curator batch until the operator's Done, or as
   a chore when a session can do the
   surrounding work, which runs its normal course to In Review.
   An operator action is an ask or an Urgent task, never a list
   entry (2026-10-26, amended 2026-10-27).

1. **Cost telemetry is reconciled against the bill.**
   Session-reported costs are projections;
   `.claude/tools/fleet_cost.py` reconciles them per complete
   UTC day against Cost Explorer with a drift flag, attributed
   per issue, seat sessions listed separately.

1. **A chore is specified in Linear.** Every chore gets a spec
   issue; a skill kicks the session off and it reads the Linear
   text as its whole brief, on Sonnet-class Bedrock, and the
   launch fails fast if the model is not enabled.

1. **Non-additive program-touching work (a frozen constant, a
   layout, an existing instruction's behavior) lands before the
   mainnet publish**; the performance-fee issue is the one ruled
   exception after C2. The publish ceremony chore carries
   the merged-at-recorded-commit check (2026-10-26). Additive
   upgrades under the retained authority (ruling 26) are possible
   after C2 and are deferred rather than rushed in; what must land
   before the publish is any change to a frozen constant, a layout
   or an existing instruction's behavior, since those are what a
   republish would mean (2026-10-30).

1. **Docs move with behavior.** A PR that changes behavior the
   architecture or interface docs describe updates those docs in
   the same PR; every dispatch carries that line in its kick-off
   (2026-10-26).

1. **A first execute rehearsal, on localnet, precedes the
   publish** as
   ceremony precondition 7: kill the ladders, close the vault,
   drain the leader through the confirm path. The C4 real-vault
   rehearsal stays (2026-10-26).

1. **Demo first, in three phases.** Throughput is bounded by how
   many sessions and ratifications run at once, so the board is
   sequenced in phases and priority follows the phase: A, the
   localnet demo, until a person swaps USDC to tGBP and back on a
   phone against the passive ladder; B, the pre-publish program
   set and the SDK freeze; C, custody and the mainnet publish with
   funds (2026-10-26).

1. **A fix that shortens every in-flight PR's path jumps the
   queue.** PR-throughput tasks get held slots ahead of the
   dispatch queue; the fleet is not resumed while a shared build
   lock makes every lint wait; every resumed or dispatched session
   is told to rebase onto current main first (2026-10-27).

1. **A blank vault name renders as `#` plus the per-market
   sequence number** on every surface, TUI and frontend alike,
   never the sector index; the leader stays visible in the vault
   dialog and the cell's hover title (product ruling, 2026-10-26).

1. **Fleet limits are the runtime config's alone.** The fleet
   total and the load gate (`DS_FLEET_TOTAL`,
   `DS_FLEET_CPU_LOAD_MAX`) are read by the dispatch tooling and
   by nothing else; no ruling, Planning-document line or scheduler
   states a value. The scheduler simply dispatches, and the
   dispatcher's refusal is how it learns a limit is reached
   (2026-10-29).

## Rulings index (beyond the numbered set)

- **The lever reservoir**: file only turn-eliminating,
  prose-subtracting or hardening levers; the payload bar is
  roughly 3k per occurrence.
- **Signing failures never halt work**, but read as an
  unpushed-state alarm.
- **No proceed-gate after bootstrap.**
- **The staging-time scope band**: 4 to 5 parts, 300 to 800
  lines, split at 1,000, with test-heavy and prose-deletion
  overages accepted as one PR when a split would ship inert code.
- **Guards stay on under auto mode.**
- **Per-tab ratification outranks planning holds.**
- ~~**Follow-ups park under the Follow-ups milestone.**~~ Struck
  2026-10-26: follow-ups file un-parked and are messaged to the
  curator.
- **The resident budget is CI-enforced**, and compression means
  **less text**.
- **Model defaults are runtime-config edits**, never repo changes.
- **Small file-disjoint parallel PRs**, overlap paid as a rebase.
- **Linear's GitHub integration makes no state change on merge** —
  the operator's Done releases dependents.

## Fleet doctrine

1. **Dispatch through the dispatcher tool**, by the scheduler, by
   the curator for the architect tasks it files (2026-10-26), or by
   the launcher, which dispatches the curator once the scheduler's
   `state of play written` arrives (2026-10-28), as new tabs in
   the dispatcher's window, with one consolidated ask; the fleet
   bound lives in the tool. Ratification and gate approvals are
   per-tab. **Todo is the dispatch authorization** (confirmed
   2026-10-28): a pre-authorized wave needs no ask, and once the
   day is started the scheduler dispatches at will on the
   priorities as they stand (per wave, each one ask, when the
   trigger answered "this wave only") — **save the start of the day**
   (2026-10-29), when `fleet resume` and the first wave wait for
   the operator's start trigger in the bootstrap ask, the launcher
   messaged before that ask. The answer is recorded as the
   document's `Fleet start:` line; a warm bootstrap finding it, with
   the fleet live, asks no trigger, and a machine restart asks it
   again. On the operator's
   start trigger, run `fleet resume` **before** dispatching new
   tasks. A session resumed outside its verb has no secrets.
1. **The antichain practice**: dispatch file-disjoint sets, and
   publish the dispatchable antichain at every bootstrap; every
   hold is a visible edge.
1. **Messaging is the event-driven backbone.** When a session owns
   the issue, the message is the amendment; when the owning
   session is gone, the amendment goes on the body. A peer message
   is never the operator's approval.
1. **A budget, not a ceiling.** The fleet's size is the machine's
   answer, never a board literal: the machine-load probe
   (`machine_load.py`) says how many more of a class fit. Every
   top-level session, this one included, spends one of
   `DS_FLEET_TOTAL`; nothing new starts while the one-minute load
   is past `DS_FLEET_CPU_LOAD_MAX`. Both are the operator's, set
   in the runtime config. A `task` is a building session, also
   refused while a pool-lock wait is at or past
   `DS_FLEET_BUILD_WAIT_S`. A free session
   (`plan`, `architect`, `explore`, a chore with no build in its
   body) is refused only on the total, memory, pressure or load —
   never swap. Builds share the pool's
   `DS_CARGO_LANES` lanes, each at most `CARGO_BUILD_JOBS`
   compilers — both the operator's too, required while the pool
   is on, and their product the fleet's compiler ceiling. On no
   reading (exit 3) fall back to the total and say so — unless
   the total itself is unset, which holds the batch. The
   digest's `## Wait` section measures whether the budget is
   right.
1. **Cost**: the spend cap is the standing fleet guard, and
   `fleet_cost.py` reconciles telemetry against the bill.
1. **Weekly skills audit**: considered weekly through the audit
   heartbeat, with the tier question asked first and the
   tool-usage lens.
1. **Verbs**: `task`, `task anthropic`, `task resume`, `chore`,
   `chore anthropic`, `explore`, `explore anthropic`, `plan` (the
   scheduler), `plan cycle`, `curate` (the curator), `launch` (the
   launcher), `architect`
   (bare number, idempotent), and `fleet`; a substrate word always
   precedes the number (`architect bedrock <n>`); `housekeeping` is
   retired, its upkeep run by the `plan` pre-launch. Audit issues
   dispatch as
   `explore`.
1. **Announce a fleet-wide toolchain change** (a global anchor or
   solana install) to every program-building session by message
   before it happens: an unannounced one once broke `check-solana`
   fleet-wide for ten minutes.

## Standing decisions

Process decisions bind here. **Product facts** are recorded here
as the scheduler's quick reference only: the named product
doc owns each, and where the two disagree the doc wins and the
line here is struck.

- **Board and process**: edges human-curated and planning-placed;
  Linear tags never in PR bodies; AI attribution never in commits
  or PRs; follow-ups to the un-parked Backlog, messaged to the
  curator; no committed
  model ids; the issue filer and appender are the zero-echo
  paths; the board tool's lookup lags on a just-created
  issue.
- **AWS** (`docs/conventions/aws-infra.md`): identity
  `dropset-bedrock-agent`; CLI-first under SSO, logging in with
  `aws sso login --use-device-code --no-browser`; the credential gate
  precedes the morning
  dispatch; AWS reports in UTC, so cost reconciliation buckets by
  UTC day.
- **CI** (`docs/ci.md`): the merge queue runs the full Rust suite
  per entry; the skill-size gate runs inside the required Lint
  check.
- **Fail-closed** (`docs/market-making.md`): the maker halts on
  Postgres loss, a pair with no live tape, a stalled estimator;
  the fence Closed state empties the FX leg; an alert never lives
  downstream of the halt it reports.
- **Estimator** (`docs/market-making.md`): publishes on its own
  15-second tick, one row per market per tick; an unfed market
  publishes its static peg at a degraded regime; tGBP-USDC
  composes from GBP/USD and the observed tGBP basis off Coinbase
  and Kraken.
- **Storage** (`docs/data-feeds.md`): a single canonical product
  id; reciprocation once, at intake; Postgres is the
  daylight-saving authority; applied migrations are immutable;
  harnesses pinned to Postgres 16 with the tag-pin guard; the
  mapped-port retry is shared.
- **Candidate primacy and feeds** (`docs/data-feeds.md`): OANDA
  tape trusted, Twelve Data untrusted, Pyth parked dark; Kraken
  pair status gates order placement, not market data;
  venue-direct, free-first, keyless where possible; dashboard
  coverage of every feed or a visible scope-out.
- **Surfaces** (`docs/dashboards.md`): Grafana reads; the TUI
  writes plus command-safety state; dashboard JSON is
  single-writer; port 3200 bind-mounts the base repo; the frontend
  deploys on Vercel with RPC, WS and
  batch size from env; the platform fee is off (zero renders no
  fee line).
- **Program and timing** (`docs/architecture.md`): loader v3; quote
  expiry dual-domain per level; v3 ELF only on mainnet, first
  deployment after the Alpenglow activation; seconds sysvar only;
  no level-price bound; one vault per leader; `set_leader`
  co-signed and emitting an event; the program id is per-cluster; reverse
  orders flip every consumed atom in the same fill, no
  half-flips; a single take is heap-bounded at about 10 legs;
  swap clients set a compute-unit limit.
- **Mints** (no product doc records these yet; this line is the
  record until one does): tGBP is classic SPL, 9 decimals; GBPA is Token-2022
  with a permanent delegate, a transfer-hook slot and a fee
  config, roster-only until the program supports Token-2022; the
  program's CPI path serves classic SPL only.
- **Build** (`docs/program-build.md`): the pinned toolchain in one
  cached Docker layer; anchor-cli 1.2.1 builds the program; v3
  only on the 4.x runtime.
- **Calendar** (`docs/market-calendar.md`): sessions-only; a
  November DST observation; a December bar-profile re-run.
