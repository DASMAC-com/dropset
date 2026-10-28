# Demo run-through

Read before an operator demo session. The demo flies the production stack
(`market-making.md` §Status, `data-feeds.md` §12); this page is only what
the smoke check cannot say for you.

## 1. Start the stack

`make demo`. It brings up the collectors and Grafana, then the frontend in
the background and the TUI in front. The TUI runs "Bootstrap all" on its own,
and every market opens **dark**. Once the log reads `Bootstrap complete`:

- `M` starts a maker on every **active** market (passive ones are skipped).
- Select tGBP-USDC and `c` to stamp its passive ladder's center. The center
  line is a chooser: `←` / `→` cycle the estimator's fair (with its regime
  and anchor), the Orca pool spot (with its age), and a typed price, and the
  drift line beneath sets them against each other in bps, to compare with
  Grafana. It opens on the estimator when its fair is live and not the static
  peg, else on Orca when its spot is fresh, else on typed. A static peg, a
  paused or a stale fair is shown and never offered; a stale Orca spot is
  shown with its age and refused at the preview.
- `T` starts every taker, if you want flow on the books.

## 2. Run the check

From a second terminal, while `make demo` is up and after
`Bootstrap complete` and `M`: `make demo-check`. Every line reads the live
stack, and the check says so in its first line. One line per surface, then
`demo stack ready` or the count of fails and skips, exiting non-zero on any
fail.

| Line                   | Pass means                                                                  |
| ---------------------- | --------------------------------------------------------------------------- |
| `mode`                 | Printed only keyless — see below                                            |
| `collector <service>`  | Container running, newest row inside its cadence's bound                    |
| `estimator`            | A fresh `fair_price` row per published market; tGBP's regime and basis      |
| `validator`            | Localnet answering, program deployed at the localnet id                     |
| `market <SYM>-USDC`    | Passive: two-sided book, rungs flip. Active: two-sided book, maker quoting  |
| `quote TGBP-USDC`      | A \$10 buy fills through `simulate_swap`, the code the frontend's WASM runs |
| `frontend` / `grafana` | `localhost:3000` / `localhost:3200` answering                               |

The quote line is chain-side: it proves the book quotes, not that the page
renders one. Step 4 is the browser half.

An active market's liveness is read off the chain, never off which makers
the TUI started. `no quote on chain yet (start the makers with M)` means its
vault has never stamped a quote: either `M` was not pressed, or the maker
started and has not quoted yet, so re-run after a few seconds. `maker stale`
means it quoted and went quiet.

`SKIPPED` is neither a pass nor a fail. With the validator down, the market
and quote lines print as `SKIPPED` with the validator's reason, so the one
fault fails once, and the closing line names every skipped surface.

`EXPECTED` is not a fail. There are two such states:

- **Keyless.** Launched without the OANDA and Twelve Data keys, the stack has
  no FX tape and the FX-fed markets halt. **An empty tGBP-USDC book on such a
  stack is correct.** Keyless is read from the keyed containers being absent
  altogether; a keyed container that exists but stopped is a crash, and fails.
  Keyless excuses only the halt: an empty book, a paused fair. A one-sided book
  or a ladder that does not flip still fails.
- **The FX weekend**, Friday 21:00 to Sunday 22:00 UTC (both daylight-time
  offsets), on the keyed collector lines only: a newest candle from the Friday
  close is excused, an older one is not. The estimator keeps publishing over
  the weekend, at `degraded` or `uncorroborated`.

The estimator line passes on a `degraded` regime, the stack's usual state for
tGBP. The regime is named so you can say why, not to fail the run.

## 3. Look at the TUI

- **Markets pane:** tGBP-USDC is selected on launch and labeled `passive` in
  yellow, with no maker ●. Every other market runs **active**, with a green ●
  while its maker runs. `P` flips the selected market's posture for the
  session.
- **Order book:** the passive ladder rests around the stamped center. `p`
  opens the passive form over the vault (drift from fair, then what rested and
  what flipped since the stamp), and `c` re-centers it through the same
  chooser — once, by hand; nothing re-centers on its own.
- **Recent fills:** taker fills land here. A filled passive rung flips to
  rest on the other side.
- **Vault names:** the accounts pane lists each vault by name, and the
  leader pane shows the leader's. The bootstrap names each vault
  `Vault <leader prefix>`; `N` renames the leader's vault, and the menu's
  name override replaces or blanks any vault's name and can lock it. A
  vault with no name shows as `#` plus its per-market sequence number
  (`#5`), never its sector index — the same on every surface.

## 4. Try it in the browser

Open `localhost:3000`, choose tGBP-USDC, and quote a \$10 buy. It should match
the check's `quote TGBP-USDC` line, unless `NEXT_PUBLIC_PLATFORM_FEE_BPS` is
set: the check quotes with no platform fee.

The localnet frontend points its RPC at `127.0.0.1:8899`, which a phone
resolves to itself, so the phone can't reach this stack. On the phone, open the
deployed frontend, connect a mobile wallet, and quote a mainnet pair (EURC,
AUDD or CADC against USDC; tGBP is not on the mainnet roster).

## 5. Tear down

Quit the TUI with `q`. That stops the validator, the bots, the explorer and the
frontend. The collectors and Grafana keep recording on purpose:
`make collectors-down` stops them, and `make clean-docker` stops everything
while keeping the recorded history.
