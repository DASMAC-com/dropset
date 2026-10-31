# Emergency runbook: stopping a mainnet vault

One page, for the moment a live vault must stop quoting. Everything here
runs from a laptop against an RPC endpoint; nothing depends on the host the
maker runs on.

In an eCLOB a vault's resting quotes outlive the bot that wrote them, so
stopping the maker stops nothing. The stop is an on-chain write from the
TUI's mainnet mode: the kill is signed by the vault's quote authority, and
close and drain by its leader.

## Before you need it

- The TUI builds on this laptop: `cargo build -p dropset-tui`. Build ahead
  of time, not under pressure.
- The SSH agent serves the admin wallet (pays the kill's fees), the
  vault's quote key and its leader key, and 1Password is unlocked. The
  cockpit refuses to start, naming the address, when one is not served.
- `tui/private-keys.env` exists, copied from
  `tui/private-keys.env.example`, with one address per key. Every
  signature prompts once.
- You are in your own terminal. The cockpit refuses an agent's shell
  (`CLAUDECODE` set), sub-agents included.
- The admin wallet holds some SOL for transaction fees.
- `DROPSET_MAINNET_RPC_URL` is exported, pointing at a provider endpoint.

## Launch

```sh
make tui-mainnet
```

The TUI checks the endpoint's genesis hash, then asks for a typed
confirmation before it draws the panel. The quote key loads at launch; the
leader key loads only when close or drain first needs it.

## The stop, in order

Every step opens a red **REAL FUNDS** prompt over a plan read fresh from
the chain: one line per roster market where the step's key holds a vault.
Read it, type `yes`, press Enter. Esc cancels with nothing sent. A step
with nothing to send opens no prompt; it prints a red log line saying why.
Each step re-reads before it sends and skips vaults already done, so
re-running one is always safe.

1. **Kill** — press `K` (or menu: `EMERGENCY: kill ladders`). Zeroes every
   level on both sides; the book empties at the next take. Reversible: the
   next profile write re-arms the ladder, and a running maker may write one
   at any time. So stop the maker if you can, and if you cannot, go
   straight on to step 2.
1. **Close** — menu: `EMERGENCY: close vaults`. Tombstones the vault:
   matching never visits it again. **Permanent**; quoting again needs a new
   vault. Depositor withdrawals stay open.
1. **Drain** — menu: `EMERGENCY: drain leader`. Withdraws the whole leader
   stake into the leader's token accounts. The TUI refuses an active vault,
   since the program's min-leader-share floor would reject a full
   withdrawal; that is why it comes after close.

## An abusive vault name

A leader names its own vault, and the name is public. The admin's answer
is menu: `EMERGENCY: override vault name`. It is not a stop and moves no
funds; it is signed by the admin wallet, never the quote key.

1. Select the market (`[` / `]`). The accounts pane lists its first four
   vaults, each as `#number`, name and leader.
1. In the form, type the vault's `#number` or its current name (a vault
   past the listed four is reached by its name), then the replacement
   name.
   Leave the replacement empty to blank the name; the vault then shows as
   `#` plus its per-market sequence number, never its sector index. The
   lock starts on, which stops the leader renaming it back; clear it
   (`←` / `→` or space) only to unlock.
1. Enter previews the change, read fresh: the vault, the old and new name,
   the lock. Type `yes` in the red **REAL FUNDS** box to send.

Unlocking takes the same override with the lock cleared. A closed vault
releases its name and its lock on its own.

## When something goes wrong

- **A vault is listed as REFUSED.** The line says why. A kill refused for
  "quote authority is not this session's quote key" cannot be signed from
  the TUI; go straight to close, which the leader signs.
- **The kill finds no vault on a market you know is quoting.** The kill
  looks vaults up by the session's quote key, so a vault opened under
  another quote key is out of its reach (it is listed as refused only once
  the leader key has loaded). Go straight to close.
- **A send fails.** The job ends in red, naming the vault. The other
  markets were still attempted. Re-run the step.
- **"A job is already running."** Wait for it to finish, or quit (`q`) and
  relaunch.
- **The quit warns that a kept writer still holds lamports.** A publish's
  handoff failed and only this session holds the writer's key: Enter retries
  the sweep ("Close stranded buffers", now selected); `q` (or Ctrl-C) again,
  with nothing pressed between, exits anyway and abandons the writer's
  balance. Esc cancels the quit and never exits, and any other key, prompt,
  click or resize re-arms the warning, so the next quit warns again.
  Mid-retry it warns too: let the retry land before quitting.
- **No transaction lands at all.** Each level's lifetime runs from the
  last reference-price write, so quotes go stale only once nothing refreshes
  it: stop the maker. That is the passive backstop, not a plan.

## Pausing across the Alpenglow activation

The Alpenglow migration (SIMD-0384) starts 5,000 slots after the feature
activates. Until a genesis certificate exists the leader packs only
votes, RPC commitment stops advancing, and blocks after the certificate
slot are deleted; Anza expects about a slot of outage but bounds none.
The maker must not be live across that boundary. This pause is a
planned stop, not an emergency one: kill only, never close or drain.

1. **Before the activation epoch begins**, stop the maker, then press
   `K` to kill the ladders. Stopping the bot alone leaves its quotes on
   the book until they expire; killing first lets a still-running maker
   re-arm them.

1. **Wait for the switch to complete.** Poll the new RPC method until it
   returns a certificate rather than `null`. A `-32601 Method not found`
   means the node predates Agave 4.3, not that the switch is pending; use
   another endpoint.

   ```sh
   curl -s "$DROPSET_MAINNET_RPC_URL" -H 'Content-Type: application/json' \
     -d '{"jsonrpc":"2.0","id":1,"method":"getAgGenesisCert"}'
   ```

   Then confirm commitment moves again: two `getSlot` reads at
   `finalized` a few seconds apart return different slots.

1. **Restart the maker.** The killed profile no longer matches the
   configured ladder, so its first tick re-arms it and re-centers.

## Rehearsal

Launch as above, press `K`, read the plan, and press Esc. Nothing is signed.
Do the same for close; **never type `yes` in a rehearsal** — close is
permanent. With every vault still live, drain opens no prompt: its red log
line refusing an active vault is the expected answer. Rehearse before every
live demo.
