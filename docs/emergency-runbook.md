# Emergency runbook: stopping a mainnet vault

One page, for the moment a live vault must stop quoting. Everything here
runs from a laptop against an RPC endpoint; nothing depends on the host the
maker runs on.

In an eCLOB a vault's resting quotes outlive the bot that wrote them, so
stopping the maker stops nothing. The stop is an on-chain write, signed by
the vault's leader, from the TUI's mainnet mode.

## Before you need it

- The TUI builds on this laptop: `cargo build -p dropset-tui`. Build ahead
  of time, not under pressure.
- 1Password is unlocked and holds the admin wallet (pays the fees) and the
  vault's leader key. Both are `op://` references, never files.
- The admin wallet holds some SOL for transaction fees.
- `DROPSET_MAINNET_RPC_URL` is exported, pointing at a provider endpoint.

## Launch

```sh
make tui-mainnet \
  TUI_MAINNET_ARGS="--wallet op://<vault>/<item>/<field> \
    --leader op://<vault>/<item>/<field>"
```

The TUI checks the endpoint's genesis hash, then asks for a typed
confirmation before it draws the panel.

## The stop, in order

Every step opens a red **REAL FUNDS** prompt over a plan read fresh from
the chain: one line per roster market where the leader leads a vault.
Read it, type `yes`, press Enter. Esc cancels with nothing sent. Each step
re-reads before it sends and skips vaults already done, so re-running one
is always safe.

1. **Kill** — press `K` (or menu: `EMERGENCY: kill ladders`). Zeroes every
   level on both sides; the book empties at the next take. Reversible: the
   next profile write re-arms the ladder, and a running maker may write one
   at any time. So stop the maker if you can, and if you cannot, go
   straight on to step 2.
1. **Close** — menu: `EMERGENCY: close vaults`. Tombstones the vault:
   matching never visits it again. **Permanent**; quoting again needs a new
   vault. Depositor withdrawals stay open.
1. **Drain** — menu: `EMERGENCY: drain leader`. Withdraws the whole leader
   stake into the leader's token accounts. It refuses an active vault (the
   program's min-leader-share floor), which is why it comes after close.
   Shares minted by the performance-fee realize that runs first stay
   behind.

## When something goes wrong

- **A vault is listed as REFUSED.** The line says why. A kill refused for
  "quote authority is not the leader" needs that vault's own quote-authority
  key; close still works with the leader.
- **A send fails.** The job ends in red, naming the vault. The other
  markets were still attempted. Re-run the step.
- **"A job is already running."** Wait for it to finish, or quit (`q`) and
  relaunch.
- **No transaction lands at all.** Each level expires on its own wall-clock
  lifetime, so quotes go stale without a write. That is the passive
  backstop, not a plan.

## Rehearsal

Launch as above, press `K`, read the plan, and press Esc. Nothing is signed.
Do the same for close and drain; drain refusing an active vault is the
expected answer. Rehearse before every live demo.
