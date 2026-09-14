# Agent-infra audit, weekly run 1 (ENG-1196)

Date: 2026-09-14. Snapshot audited: main at `649ff6f6` (verified
unchanged on `origin/main` for every audited path at filing time).
Method: three producer sub-agents (conventions; review-pr + init-pr;
the seventeen remaining skills), one adversarial verifier over the
fourteen riskiest claims (12 confirmed, 2 adjusted in wording only,
0 refuted), plus transcript-measured usage. Research-only session:
no PR, no skill or convention edits. Findings are keyed to headings
and quoted phrases, not line numbers — seven sessions were editing
this surface while it was audited.

## Re-measured baseline (standing question: is the surface smaller?)

It is not. Eleven days after the 2026-09-03 baseline, every class
grew:

| Surface | 2026-09-03 | 2026-09-14 | Delta |
| --- | --- | --- | --- |
| CLAUDE.md | 27KB (~6.6k tok) | 32.2KB (~8.2k tok) | +19% |
| Convention docs (12) | 292KB | 379KB | +30% |
| Skills (20 → 19) | 572KB | 659KB | +15% |
| Tools (lines, sans tests) | ~17.4k | 22.4k | +29% |

Largest single items: review-pr 278KB, context-economy.md 82KB,
linear-automation.md 80KB, local-integrations.md 77KB, init-pr 81KB.
Fidelity is the objective per the 2026-09-11 re-aim; the trajectory
is reported because the recurrence unit's charter asks for it.

## Usage (21 days of transcripts, model-invoked + user-typed)

Hot: init-pr 106, session-metrics 77, review-pr 72, proceed 68,
pr-title-description 60, commit-changes 16, plan 15, housekeeping 13,
wrap 11, trim-context 10. Cold: merge-tasks 7, architect 7,
purge-conversations 2, linear-task 2, root-cause 1, audit-scope 1,
audit 0, cspell-audit 0, harden 0.

Tool calls (top): run_quiet 2348, search_source 1951, lint_paths 623,
trim_levers 599, read_result 386, review_diff 379, board_batch 352.
Zero-invocation committed tool: render_review — but it is NOT dead
(see "killed candidates" below).

Judgment on the cold skills: `audit` is the one deletion-shaped
candidate (see the merger proposal). `harden` is cold but is the
named consumer of session-metrics' candidate ranking — keep.
`root-cause`, `linear-task`, `cspell-audit` are small and load-bearing
when needed — keep.

## Confirmed findings

Filed as parked `Claude:` issues (Todo + `Claude meta`), themed by
surface, fingerprints inside each issue. Summary, ranked by severity:

### Fidelity — contradictions and retired mechanisms stated as live

1. review-pr step 21 says the Linear/GitHub integration "will have
   moved the issue to Done on this signal" — the retired auto-Done,
   contradicted twice in the same step. (in-flight: eng-1303)
2. review-pr claims a terminal mark-Done `AskUserQuestion` that
   exists nowhere in its 21 steps; the closing checklist has three
   boxes and no Done gate. (eng-1303)
3. review-pr step 3 tells deferred checklist ticks to "ride the
   In-Review write's patch array"; step 19's write is
   `board_batch.py state`, which takes no body. A reader must either
   drop the ticks or make the extra write step 3 forbids. (eng-1303)
4. The audit heartbeat has no record anywhere in skills or
   conventions of the product-audit suspension (lean-MVP 2026-09-07)
   or the weekly agent-infra exception (2026-09-11); `plan` step 1
   instructs filing against the full registry roster. The one
   net-positive-lines fix in this set (~+8).
5. commits-and-prs.md names "housekeeping's audit kickoff" as the
   one arg-gated handoff exception — housekeeping runs no audit and
   has no such arg (its args are `cspell` and `interactive`).
6. session-metrics states the eased filing bar ("no size gate — the
   producer records unconditionally"); the operator later tightened
   it to recurrence / instruction / large-sink. Highest-traffic
   skill in the corpus. (eng-1364)
7. session-metrics and trim-context both describe the retired
   assembly precondition as the live mechanism that "replaced" the
   meta-batch edge.
8. housekeeping step 7a asserts firming "never prunes" — the
   convention and `allowlist.py` both say subsumed entries are
   pruned in the same pass — and names a "firming pass" that retired
   with firm-perms. (eng-1290)
9. linear-task claims `audit`, `audit-scope`, and `housekeeping`
   "run this same probe" (`linear_issue.py find`); all three
   prescribe the ~1.9k-token MCP `list_issues` call instead. Fix the
   three call sites, or the claim.
10. merge-tasks says housekeeping "proactively proposes such merge
    groups"; housekeeping's step 6 is literally "RETIRED" and
    assigns the job to `plan`.
11. housekeeping step 3's fenced cspell filing hard-codes
    `--state Backlog` with no `Claude:` prefix, contradicting its
    own intro and cspell-audit's Backlog-or-parked rule (the common
    case — cfg/dictionary.txt — is the parked one). (eng-1290)
12. cspell-audit calls itself part of housekeeping's periodic pass
    (it is opt-in) and mandates the Grep tool unconditionally where
    shell-commands.md documents builds without it.
13. review-pr's reduced-tier prose: entry gate says a reduced tier
    "still includes an independent cross-check"; step 5's trivial
    tier explicitly skips it. Operator consent to a tier is
    uninformed either way. (eng-1303)
14. docs-and-style.md says housekeeping runs the cspell check every
    pass and files drift to Backlog — both halves wrong.
15. Meta-prefix emitter lists in CLAUDE.md and linear-automation.md
    omit trim-context, which both files elsewhere describe emitting
    `Claude:` titles. (CLAUDE.md in-flight: eng-1378/eng-1367)
16. linear-automation.md instructs keeping `META_BASES` "in sync"
    with a four-member list; the tuple has three members (no
    CLAUDE.md entry). Impact bounded to legacy Touches bodies.
17. context-economy.md points readers at "the /verify and /run
    flows"; no such verbs exist on this surface.
18. review-pr: three off-by-one step cross-references (step-1
    rebase / step-10 tests / step-11 push / step-18 write).
    (eng-1303)
19. init-pr step 0b: "reached step 7 of 12" — the skill has 14
    steps.
20. trim-context keeps a "one-time legacy drain" appendix for the
    retired inbox document that three other places assert is gone.
21. architect defers shared-block de-duplication "until the
    template extraction lands"; the render machinery landed and the
    same file already carries a render region. (eng-1367)
22. linear-automation.md's consolidation-cost passage still argues
    from the retired always-one-task fold ruling, ~800 lines after
    the same doc retires it. (adjacent: eng-1378)

### Compactness (co-benefit; argued from duplication, not tokens)

23. init-pr's "Implementing the task: keep context discipline on" —
    655 lines, 37% of the skill, a restatement of
    context-economy.md down to shared measurements (the 91%
    screenshot figure, the deviceScaleFactor passage, five parallel
    headings). Replace with the init-pr-specific residue plus one
    pointer (~500+ lines).
24. review-pr self-reference tax: four steps carry superseded-shape
    narrations (~110 lines). (eng-1303)
25. init-pr step 0b re-derives the signing dispatch that
    `init_pr_branch.py` computes and documents (~70 lines).
26. housekeeping: three stacked retirement narrations (~65 lines).
27. migration_collisions.py explained at length in two skills;
    single-owner it in review-pr (~55 lines). (eng-1303 half)
28. commit-changes: ~40 lines of checkpoint-gate history around two
    `tree_fingerprint.py` commands. (eng-1299)
29. The rebase-cost "cents" figure asserted as fact three times in
    linear-automation.md while context-economy.md flags it as the
    one unmeasured load-bearing estimate — shrink to pointers.
    (eng-1364 adjacent)
30. The fingerprint-dedup block (same measurements) stated in full
    in both linear-automation.md and context-economy.md.
31. CLAUDE.md index parentheticals re-telling retirement chains the
    linked doc owns (~12 lines from the most-replayed file).
    (eng-1378/eng-1367)
32. Five hand-copies of the shell-discipline note and three of the
    printenv note across skills, with drifted citation targets —
    two render regions.

### Structural proposal

33. Fold `audit` into `audit-scope --rotation N` and delete the
    skill: 0 invocations in 21 days, ad-hoc-only by its own
    statement, subject matter suspended, and mostly a second copy of
    audit-scope's dedup/fold/file path (~450 net lines). The unique
    ~30 lines (random registry sampling, the 7-unit interleave)
    move into audit-scope. Taste-tier: plan adjudicates.

## Deterministic-procedure ranking (plan-7 column)

Fraction of prose that is procedure a committed tool could run, and
the top candidates by prose deleted. Existing coverage first:
ENG-1386 already claims the init-pr scripted bootstrap and
merge-watch notification dismissal — cite it, don't refile.

| Candidate | Skill / section | ~Lines |
| --- | --- | --- |
| lint dispatch (extend `lint_paths.py --changed`) | review-pr "Run lint." | 380 |
| `merge_outcome.py` (settle window, probe, 3-way verdict) | review-pr step 21 | 300 |
| CI-wait prose (tool exists; prose re-narrates it) | review-pr step 17 | 230 |
| `trim_levers.py fold-read` composite | trim-context step 2 | 130 |
| signing + scope verdict in `init_pr_branch.py` | init-pr step 0b | 110 |
| `cspell_place.py audit` (classify + placement scan) | cspell-audit steps 1–4 | 95 |
| `linear_issue.py fingerprint` (pure string transform) | audit + audit-scope | 60 |
| checkpoint gate (tool exists: `tree_fingerprint.py`) | commit-changes step 2 | 40 |
| relations union in `merge_tasks.py assemble` | merge-tasks step 3 | 25 |

Per-skill procedure fractions (est.): cspell-audit ~55%,
trim-context ~45%, init-pr ~40%, audit ~35%, review-pr ~35%,
audit-scope ~30%, commit-changes ~30%, purge-conversations ~25%,
merge-tasks / housekeeping / session-metrics / linear-task ~20%,
wrap / pr-title-description ~15%, plan / harden ~10%, architect ~5%,
root-cause / proceed ~0%.

## Killed candidates worth recording

- `render_review.py` is NOT dead: it is driven by the deck-export
  session workflow (`decks/scripts/export-deck.mjs`), not by a
  skill, and both doc references are accurate. Its name and
  location mislead an auditor of the review surface; at most a
  rename, not a deletion.
- review-pr's merge-queue-membership prose is correct at HEAD
  (states the mergeQueueEntry-only rule and the ambiguous-null
  trap); the tiebreak defect was already fixed via the 1345 change.
- All 112 convention citations resolve (`convention_refs.py`); every
  defect found is semantic, the class that tool cannot see.
- Housekeeping's worktree-prune criterion, wrap's In-Review rule,
  and the audit-registry refresh claim all check out.
- Every tool flag cited by review-pr / init-pr exists in the
  argparse at HEAD.

## Recurrence

The registry row exists (`agent-infra (agent-infra, med)`); the
registry carries no cadence column — the weekly cadence lives in the
Planning document's audit-state table, which the planning session
keeps. Next run's standing question: the baseline table above,
re-measured, plus whether the count of live contradictions (22
fidelity findings this run) fell.
