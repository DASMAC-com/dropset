# Demo run-through

Read before an operator demo session. The demo flies the production stack
(`market-making.md` §Status, `data-feeds.md` §12); this page is only what
the smoke check cannot say for you.

## 1. Start the stack

`make demo`. It brings up the collectors and Grafana, then the frontend in
the background and the TUI in front. The TUI runs "Bootstrap all" on its own,
and every market opens **dark**. Once the log reads `Bootstrap complete`:

- `M` starts a maker on every **active** market (passive ones are skipped).
- Select tGBP-USDC and `c` to stamp its passive ladder's center. A blank
  center is the estimator's fair when `DROPSET_TUI_DATABASE_URL` is set to
  `postgres://dropset_ro:dropset_ro@127.0.0.1:5432/dropset`. Otherwise type
  one.
- `T` starts every taker, if you want flow on the books.

## 2. Run the check

From a second terminal: `make demo-check`. One line per surface, then
`demo stack ready` or the count of fails, with exit code 1 on any fail.

| Line                   | Pass means                                                                  |
| ---------------------- | --------------------------------------------------------------------------- |
| `mode`                 | Printed only keyless — see below                                            |
| `collector <service>`  | Container running, newest row inside its cadence's bound                    |
| `estimator`            | A fresh `fair_price` row per published market; tGBP's regime and basis      |
| `validator`            | Localnet answering, program deployed at the localnet id                     |
| `market <SYM>-USDC`    | Passive: two-sided book, rungs flip. Active: two-sided book, maker quoting  |
| `quote TGBP-USDC`      | A \$10 buy fills through `simulate_swap`, the code the frontend's WASM runs |
| `frontend` / `grafana` | `localhost:3000` / `localhost:3200` answering                               |

`EXPECTED` is not a fail. There are two such states:

- **Keyless.** Launched without the OANDA and Twelve Data keys, the stack has
  no FX tape and the FX-fed markets halt. **An empty tGBP-USDC book on such a
  stack is correct.** The keyed collectors read as not started.
- **The FX weekend**, Friday 22:00 to Sunday 22:00 UTC, when a keyed
  collector's newest candle ages past its bound.

The estimator line passes on a `degraded` regime. The regime is named so you
can say why, not to fail the run.

## 3. Look at the TUI

- **Markets pane:** tGBP-USDC is selected on launch and labeled `passive` in
  yellow, with no maker ●. Every other market runs **active**, with a green ●
  while its maker runs. `P` flips the selected market's posture for the
  session.
- **Order book:** the passive ladder rests around the stamped center. `p`
  opens the passive form over the vault (drift from fair, then what rested and
  what flipped since the stamp), and `c` re-centers it.
- **Recent fills:** taker fills land here. A filled passive rung flips to
  rest on the other side.

## 4. Try it in the browser

Open `localhost:3000`, choose tGBP-USDC, and quote a swap. The quote should
match the check's `quote TGBP-USDC` line. The localnet frontend points its RPC
at `127.0.0.1:8899`, which a phone resolves to itself, so the phone half of a
demo runs against mainnet, not this stack.

## 5. Tear down

Quit the TUI with `q`. That stops the validator, the bots, the explorer and the
frontend. The collectors and Grafana keep recording on purpose:
`make collectors-down` stops them, and `make clean-docker` stops everything
while keeping the recorded history.
