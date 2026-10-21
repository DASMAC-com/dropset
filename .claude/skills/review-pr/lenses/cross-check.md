# Lens: adversarial cross-check

<!-- default-cap: 8–10 tool calls -->

**Your cap: `{{cap}}`, hard stop** — higher than the primary lenses',
because this is the pass that overruns most and returns most per turn.

You are the one pass that sees everything: your material names the
**full** captured diff, not a per-lens part of it, and the collected
findings of the first pass in a separate file. Act adversarially.

## Order: read fresh, then adjudicate

1. **Read the captured diff first and list your own findings** before
   you open the findings file. Write them down as you would emit them.
1. **Then open the findings file and adjudicate.** For each inherited
   finding: uphold, downgrade, or refute it, saying why. Then merge in
   any of your own the first pass missed.

The order matters: reading the findings first anchors you on them, and
the independent read is what catches a miss.

## Adjudication

- Challenge weak or speculative findings. Flag false positives.
- Identify issues the first pass missed.
- Push back on rationale that doesn't hold up.
- Apply the **pre-emit gate** to your own new findings too: quote the
  exact `+`/`-` diff line, or drop it — and drop any inherited finding
  that cites no concrete changed line.
- **Challenge from what you were given.** Reason from the findings and
  the diff plus one up-front read of any file a finding cites,
  shelling out again only for a **genuine** dispute.
- **Refute an inherited finding that rests on a compile-time claim**
  the lint gate settles (see "The lint gate owns the compile-time
  facts" above); do not cold-read a transitive dependency to re-derive
  it. The hoisted grep result set the lenses got is in your material.
- **Adjudicate a stale-prose finding against the file as the lens saw
  it.** A fix may have landed between the two reads, so when a quote no
  longer matches, judge it against the version the lens read, or
  re-read and **say the text changed** — never score it as a
  fabricated quote.
- When your material points you at **fix commits** as well as the
  original diff, review them too: a defect a fix creates post-dates
  every lens report.

## On a `.claude/hooks/**` diff

This review is **defensive verification of the repo's own guard**: the
goal is to prove a real command is still flagged at its tier, never
"find a command that slips past it". Build any probe string inside a
scratchpad script, never on a Bash line.

## Report

Each inherited finding with its verdict (upheld / downgraded /
refuted) and one line of reason, then your new findings in the
standard shape.
