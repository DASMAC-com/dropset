# `review-pr` history ledger

The measured incidents behind the rules in [`SKILL.md`](SKILL.md),
kept here so the entry file states each rule once. Provenance only;
not loaded on invocation. The entry file grew 60k → 84k → 212k → 292k
bytes from July to October across 47 commits with nothing pushing back
(measured by the design thread that filed the size gate), which is what
the resident-size gate (`docs/conventions/context-economy.md` →
"Resident size has a hard cap") was built to stop.

## Step 5: slicing the review diff

- **Generated output dominating the diff.** A conformance-vectors PR,
  before `sdk/conformance` joined `DIFF_EXCLUDES`: a 5783-line diff of
  which ~3460 lines were vector JSON, so even the tests slice came out
  at 4872 lines, and the category split could not isolate the 1532
  hand-written lines either. The two lenses handed the full diff were
  that review's two most expensive, at 2.6–2.9x the cheapest, and the
  ordering tracked handed-in size almost monotonically. Both had been
  told to "skim past" the JSON — prompt discipline standing in for a
  slice that should not have contained it.
