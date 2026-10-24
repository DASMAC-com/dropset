<!-- cspell:word antichain -->

<!-- cspell:word GBPA -->

# `plan` rulings and doctrine

The operator's standing rulings, the fleet doctrine and the
standing decisions a planning session obeys. They are
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
   subscription, `plan` on Bedrock (its lever fold on the executor),
   `explore` on Bedrock with executor-tier sub-agents. Executor:
   task workers on Bedrock (`housekeeping`, once the seat exception
   here, is retired). Helper: `chore`
   sessions, Sonnet-class, Bedrock by default, pinned with the 1M
   suffix in the runtime config.
   Model ids live only in the operator's runtime config; unset
   refuses the launch.

1. **Next means dispatchable right now.** A planning hold is a
   blocking edge the operator can see, never prose-only state.
   Planning places and removes edges; automated filers never
   place one. (`SKILL.md` step 2.)

1. **Meta discretion — the necessity kill gate.** The promotion
   review is a two-list act: a file-disjoint slate and a kill
   list; close on doubt. The one-time cull of the whole
   blocked-plus-parked meta stock ran on 2026-10-15 (34 closed,
   12 folds, survivors parked); what stands is the two-list
   review at every full bootstrap.

1. **Headroom ratchet.** A shrinking PR commits its ceiling at
   size times 1.10; bytes-per-token measured at 2.76 to 2.78.

1. **Queue watch.** A review session never hands the merge wait
   to the operator.

1. **AWS.** Every suggested login lets the operator pick the
   identity: the login gate runs `aws login --remote` (the
   paste-the-code path; `aws login` has no device-code flag), and a
   bare `aws sso login` keeps `--use-device-code`. The spend cap
   is live at 2,000 dollars per rolling 24 hours;
   **on a sudden Bedrock access-denied, check the kill switch
   before credentials.** Key rotation is yearly.

1. **Mid-flight splits: one task per PR.** A worker that finds
   its issue too large finishes one self-contained PR, messages
   planning the remainder as pullable units, and ends its
   session; planning files them and **re-points the blocking
   edges to the chain's tail**, so ratifying the first issue does
   not release work that depends on the whole. (`SKILL.md`
   step 5.)

1. **Meta stack deliberateness.** Meta work is dispatched
   deliberately, not as a constant stream.

1. **Atomic session close.** Mark Done first, remove the
   worktree second; the close tool is the session's last call.

1. **AWS operator chores are one attended session.** A program
   probe (the loader-v4 probe) stays a separate program item.

1. **Fan-out gate.** Invoking a verb pre-approves its
   regular sub-agent fan-out — `review-pr`'s reviewers,
   `audit-scope`'s cross-check, `plan`'s bootstrap lever fold.

1. **Architect read-back.** Fold the operator's comments into the
   spec in place; the spec file in an architect worktree is
   expendable once the rulings are on the issue. The spec lives at
   the worktree root, never under a repo-shaped path. The
   read-back ends in the operator's explicit yes (2026-10-19),
   now given per task on tasks filed into Backlog as the design
   settles.
   **An architect never moves a task it filed to Todo**
   (2026-10-22): the operator reviews the filed tasks in its tab
   and makes the move, or explicitly directs it there — never on
   the architect's own judgment of ratification.

1. **Planning is read-only toward the repo.** A planning session
   writes Linear state only; a fast-forward of the base checkout
   is not a planning edit. Edits to the operator's runtime config
   on the operator's word are outside the repo and allowed, and so
   are memory notes, which live outside it.

1. **The deploy ceremony is code.** Committed commands with
   deterministic 1Password secret references; the agent never
   holds a secret. The program keypair is stored as a
   128-character hex string of the 64-byte keypair; the cockpit
   decodes hex, base58 or JSON in memory.

1. **Session metrics is not an ask.** It files unprompted
   immediately before the atomic-close question; nothing files if
   the PR leaves the queue. Cost is reported right before the
   close.

1. **Housekeeping never costs a tab visit.** Upkeep runs as a tool
   inside the plan launch with its report in the bootstrap;
   destructive items are asked once in the planning tab; the
   trim-lever fold runs as a sub-agent of the planning session.
   Neither costs a tab.

1. **Dispatch never steals focus, and no notification for
   self-waiting.** A yellow tab is a trigger problem, not a window
   problem: it turns yellow only for a request actually waiting on
   the operator, never for classifier-approved requests in rapid
   succession. The debounce is an environment variable with a
   5-second default.

1. **No hardcoded 200ms.** Expiry defaults derive from the
   observed slot pace.

1. **Meta freeze until C2.** No new meta promotions beyond what
   the operator names; the named exceptions are state, listed in
   the document's direction section.

1. **Parked hygiene is a rhythm.** Every full bootstrap says each
   parked milestone's count with its direction, machine-written by
   upkeep; the lever pool folds there (ruling 23), and Follow-ups
   and Audit findings get count-and-offer lines (`SKILL.md`
   step 8).

1. **Alpenglow proof is explicit.** The v3 port closes only with
   a recorded end-to-end deploy on an Alpenglow-enabled 4.3
   localnet; C2 is the real-cluster proof; a pre-C2 devnet
   rehearsal is asked, not assumed.

1. **The tGBP basis is observed, not pinned.**

1. **Trim levers must not accumulate.** Nearly every PR files
   levers, acceptable only while the daily startup consumes them:
   the bootstrap's fold sub-agent folds the milestone every
   morning, the meta review kills on doubt, and every full
   bootstrap says the lever count with its direction since the
   previous stamp, as upkeep writes it. A pool that grows
   two bootstraps running is a filer defect (raise the reservoir
   bar), not a backlog. A couple hundred dollars a day is
   acceptable spend; unbounded growth is the only failure
   watched.

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
   documented early-mainnet build, dropped only at the final
   immutable deploy.

1. **Operator-executed items are not tasks.** An item executed in
   a vendor dashboard, credentials or deploy env is never
   dispatched; it parks under Follow-ups and goes on the
   document's operator-actions list by hand.

1. **Cost telemetry is reconciled against the bill.**
   Session-reported costs are projections;
   `.claude/tools/fleet_cost.py` reconciles them per complete
   UTC day against Cost Explorer with a drift flag, attributed
   per issue, seat sessions listed separately.

1. **A chore is specified in Linear.** Every chore gets a spec
   issue; a skill kicks the session off and it reads the Linear
   text as its whole brief, on Sonnet-class Bedrock, and the
   launch fails fast if the model is not enabled.

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
- **Follow-ups park under the Follow-ups milestone.**
- **The resident budget is CI-enforced**, and compression means
  **less text**.
- **Model defaults are runtime-config edits**, never repo changes.
- **Small file-disjoint parallel PRs**, overlap paid as a rebase.
- **Linear's GitHub integration makes no state change on merge** —
  the operator's Done releases dependents.

## Fleet doctrine

1. **Planning-only dispatch**, as new tabs in the planning window,
   with one consolidated ask; ratification and gate approvals are
   per-tab. Run `fleet resume` **before** dispatching new tasks. A
   session resumed outside its verb has no secrets.
1. **The antichain practice**: dispatch file-disjoint sets, and
   publish the dispatchable antichain at every bootstrap; every
   hold is a visible edge.
1. **Messaging is the event-driven backbone.** When a session owns
   the issue, the message is the amendment; when the owning
   session is gone, the amendment goes on the body. A peer message
   is never the operator's approval.
1. **Cost**: the spend cap is the standing fleet guard, and
   `fleet_cost.py` reconciles telemetry against the bill.
1. **Weekly skills audit**: considered weekly through the audit
   heartbeat, with the tier question asked first and the
   tool-usage lens.
1. **Verbs**: `task`, `task anthropic`, `task resume`, `chore`,
   `chore anthropic`, `explore`, `explore anthropic`, `plan`,
   `plan cycle`, `architect`
   (bare number, idempotent), and `fleet`; a substrate word always
   precedes the number (`architect bedrock <n>`); `housekeeping` is
   retired, its upkeep run by the `plan` launcher. Audit issues
   dispatch as
   `explore`.
1. **Announce a fleet-wide toolchain change** (a global anchor or
   solana install) to every program-building session by message
   before it happens: an unannounced one once broke `check-solana`
   fleet-wide for ten minutes.

## Standing decisions

Process decisions bind here. **Product facts** are recorded here
as the planning session's quick reference only: the named product
doc owns each, and where the two disagree the doc wins and the
line here is struck.

- **Board and process**: edges human-curated and planning-placed;
  Linear tags never in PR bodies; AI attribution never in commits
  or PRs; follow-ups to the Follow-ups milestone; no committed
  model ids; the trim-levers writer and the issue appender are the
  zero-echo paths; the board tool's lookup lags on a just-created
  issue.
- **AWS** (`docs/conventions/aws-infra.md`): identity
  `dropset-bedrock-agent`; CLI-first under SSO, logging in with
  `aws login --remote`; the credential gate precedes the morning
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
