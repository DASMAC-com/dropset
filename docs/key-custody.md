<!-- cspell:word CMK -->

<!-- cspell:word dalek -->

<!-- cspell:word IMDS -->

<!-- cspell:word mlock -->

<!-- cspell:word newtype -->

<!-- cspell:word OANDA -->

<!-- cspell:word zeroization -->

<!-- cspell:word zeroize -->

# Maker Key Custody

This document answers two questions, and only two:

1. **Can key material leak out of the maker as it stands today?** The
   maker signs on a cadence and the stack is open source, so anyone can
   read the exact code we run. §1 traces the signing key end to end and
   records what was found — including one real defect and several
   confirmed negatives.
1. **What must be true before a real signing key is provisioned on
   AWS?** Nothing is deployed yet (§2), which is precisely why the rules
   belong here now: they are cheap to adopt while the deployment is
   still being designed and expensive to retrofit afterwards. §3 is
   those rules.

The audit half is verified against the tree at the commit this document
lands on, and §1 cites the line numbers it rests on. Those anchors are a
snapshot: re-derive them rather than trusting them on a later read. The
spec half is a standing posture, not a one-shot — §5 names the triggers
that re-open it, and §6 is the recurring pass.

## 1. Current state: the signing key today

### 1.1 The localnet guard, and exactly what it checks

`bots/maker-bot/src/main.rs:172-174` guards before it loads the signing
key:

```rust
chain::assert_localnet(&client)?;
let leader = solana_keypair::read_keypair_file(&args.leader_key)
```

The guard is line 172 and the key read is line 173, so the ordering is
load-bearing and correct: a guard placed after the read would already
have the secret in memory before deciding it was misconfigured. That
ordering should stay.

**Be precise about what the guard does, because §5 rests the entire
current posture on it.** `chain.rs:59-86` fetches the cluster's genesis
hash and compares it against three constants — mainnet-beta, devnet and
testnet — returning `None` for anything else. It is a **denylist that
fails open**: it refuses three known genesis hashes and passes
everything else. A private validator, a mainnet fork, a new public
cluster, or another SVM chain would not be refused.

Two consequences, in both directions:

- The accurate claim is "the maker refuses the three public Solana
  clusters", not "the maker cannot run anywhere but localnet". The
  three do cover every public Solana cluster that exists today, so the
  practical protection is real — but it is a property of an enumerated
  list, not of the mechanism, and it does not extend to a cluster that
  does not exist yet.
- The guard is **stronger** than a URL check in one respect worth
  keeping: because it keys on the genesis hash rather than the RPC
  host, it still trips when a public cluster is tunnelled through a
  loopback address by a port-forward or proxy.

So there is currently no mainnet signing path in the maker. Every risk
below is latent rather than live, and the deployment work is what makes
it live.

### 1.2 The signing key is a committed localnet file

The default leader key is `keys/EEEE.json`, one of fourteen keypairs
committed to the repository. This is deliberate and already documented
— `keys/README.md` carries an explicit warning that the secrets are in
plain text, that anyone can sign for them, and that they must never be
funded on devnet or mainnet.

Recording it here because it is the single most important fact for §3:
the mechanism by which the maker obtains a key today is *reading a file
that is in the open-source repository*. The deployed maker must not
obtain its key by any path that resembles this one, and the transition
between the two is the seam the spec governs.

### 1.3 Where the secret actually goes

Every consumer of the leader key in the maker, enumerated:

| Form                   | Sites                                             | Reaches                                                    |
| ---------------------- | ------------------------------------------------- | ---------------------------------------------------------- |
| `&ctx.leader` (secret) | `tasks.rs` 666, 991, 1103, 1127, 1151, 1180       | `chain::` signing calls only                               |
| `leader.pubkey()`      | `tasks.rs` 622, 848; `main.rs` 187, 192, 195, 287 | balance check, airdrop log, fill subscription, state reads |

The secret half is passed to `chain::` signing calls and to nothing
else. Public keys are the only form that leaves this set.

State the bound with the result, because it is what makes the negative
safe to inherit: this was established by enumerating **producers**
inside the maker binary, not by reading the TUI, telemetry, or
parameter-channel consumers. The inference is sound — a value that is
never passed out cannot be received — but it is a fact about this
crate's call sites, not a general clean bill for key custody.

Three structural facts support it:

- `Context` (`context.rs:77-92`) does **not** derive `Debug`. A derived
  `Debug` on a struct holding a `Keypair` is the classic
  accidental-disclosure path — a single `{:?}` in an error branch would
  reach for it. It is absent and should stay absent, which is worth a
  comment on the struct: the next person to add a field may reach for
  `#[derive(Debug)]` without knowing what else is in there.
- Signing funnels through one private `send` (`chain.rs:289`), the
  closest thing to a single seam the maker currently has.
- **The inner type redacts, upstream — and this is not our property.**
  `solana_keypair::Keypair` *does* derive `Debug`, so the wrapper ban
  above is not what saves us. What saves us is that
  `ed25519_dalek::SigningKey` hand-writes a `Debug` emitting only
  `verifying_key` and calling `.finish_non_exhaustive()`, with an
  inline comment stating it avoids printing the secret key; verified in
  both vendored versions, 2.1.1 and 2.2.0. **The bound is the point:
  this is an upstream implementation detail at a pinned version.** It
  is true of this tree, not of any tree, and a dependency bump can
  silently remove it — which is why §3.3 turns it into something we own
  and §5 makes it a re-audit trigger.

### 1.4 Resolved: the key was cloned per market

**Status: fixed.** Recorded rather than deleted because §3.1 is written
against it, and because §6's signing-seam re-check is what would catch
it coming back.

The defect, as found: the startup loop built one `Context` per market
and gave each its own `leader.insecure_clone()`, so the 32-byte secret
existed in memory once *per market*, in long-lived structs, for the
life of the process. The method that put it there is named
`insecure_clone` by upstream precisely because it defeats the
single-owner discipline the type is designed around. The context doc
comment already said the markets "share a leader", which was true of
the *identity* and false of the *storage*.

The key is now read once into an `Arc<Keypair>` that every context
shares (`context.rs:92`), and the `insecure_clone` call is gone. The
secret exists once in the process for any roster size, and the type is
what holds the line: cloning the handle to build the next context
cannot duplicate key material, so the N-copies shape cannot return by
accident.

This is the storage half of §3.1 and not the whole of it — contexts
still receive something that *is* a `Keypair` rather than something
that merely signs. Narrowing that to a signer interface is the part
still outstanding, and it belongs with the deployment work, where a
non-file key first exists.

### 1.5 CI leak surface: measured clean

The concern is that a key-shaped file reaches a build log, an action
artifact, a cached layer, or a published image.

The workflows do materialize a keypair — `make program-keypair` copies
`keys/AAAA.json` into `target/deploy/` so `declare_id!` and anchor's
build-time check agree. That is the *program* keypair, already public
in the repository, so it discloses nothing new.

What matters is that no CI path sweeps up the directory it lands in.
Every artifact and cache path is file-scoped to a compiled binary:

```text
target/deploy/dropset.so
target/deploy/dropset_ref.so
```

No path covers `target/deploy/` wholesale, so the keypair beside them
is never cached and never uploaded. The repository secrets in use
(container registry credentials, the GitHub token) are referenced
through `${{ secrets.* }}` in `env:` and `with:` blocks and are never
echoed into a `run:` step.

**This is a negative result about today's workflows only.** It is not a
guarantee about a future deploy workflow, which does not exist yet —
see §3.6.

## 2. What is not built yet

`infra/aws/` contains exactly three CloudFormation templates —
`network.yml`, `iam-baseline.yml`, and `cloudtrail.yml` — and a
repository-wide search for `AWSTemplateFormatVersion` returns only
those three. There is no compute stack, no secrets stack, and nothing
the maker could be deployed onto.

The two pieces of work that would change this are the maker secrets
provider (Secrets Manager plus an IAM execution role) and the mainnet
maker compute stack; both are filed and unstarted.

So the deployed-state half of this subject **cannot be audited** — the
thing to audit does not exist. That is the reason this document is a
spec rather than a second audit, and the reason the verification pass
against real infrastructure is filed separately, blocked on those two.

## 3. Rules for the deployed signing key

These are requirements on the deployment work, written before it is
built so it can be built to them.

### 3.1 One signer, one seam — and a seam that bounds what it signs

The process must hold exactly **one** owner of the secret, behind a
narrow interface that exposes signing and never exposes bytes. Callers
receive something that can sign; they do not receive a `Keypair`.

The storage half of this has landed (§1.4): the per-market
`insecure_clone` is gone and the contexts share one handle. The
interface half has not — that handle is still an `Arc<Keypair>`, so a
caller holding a context can reach the bytes. Closing that is
deployment work, since a signer interface is only meaningful once
there is a non-file key behind it.

**That is necessary and it is not sufficient, and the gap is the most
important rule in this document.** A narrow seam prevents key *theft*.
It does nothing about key *use* — and against the attacker §3.4 and
§3.5 both assume, one with code execution on the host, a signer that
will sign whatever it is handed is close to equivalent to handing over
the key. Confidentiality is not the property that matters most here.

So the seam must also **bound what it will sign**. It accepts only
transactions whose instructions target the dropset program and the
specific entry points the maker legitimately needs — today
`set_reference_price`, `invalidate_reference_price`, and
`set_liquidity_profile` — and refuses anything else, including a
transfer, an authority change, or an instruction against another
program. A payload policy is cheap to state now and expensive to
retrofit into a signing interface that was built without one, and it is
one of only two controls in this document that survive full host
compromise; the other is the program-level bound in §3.4.

### 3.2 The key never lands on disk, and never exists outside the provider

The deployed maker must not read its signing key from a file. No
keypair file in the image, in a bind mount, in the compose shape, or
written to a temporary path at startup. The key is fetched from the
secrets provider into memory by the process that uses it.

**Govern the key's origination, not only its read path.** §1.2 rejects
obtaining a key by reading a repository file, but a key generated on a
workstation and pasted into the provider has transited that machine's
memory, clipboard, shell history and terminal scrollback — the same
exposure in a different location. So: the production key is generated
where it will live, and never exists in plaintext outside the provider
and the running process. Origination and rotation (§3.4) are one rule
seen from two ends.

The `--leader-key <path>` flag is a localnet affordance and must not be
the production input. Note it carries a **default** of `keys/EEEE.json`,
so removing the flag means removing the default with it. How that is
enforced depends on an open question (§7): if the localnet guard becomes
a compile-time feature there is a production build for the flag to be
absent from, and if it stays a runtime check there is not — in which
case this rule must be restated as a runtime refusal rather than an
absent flag. State which of the two applies when that question is
settled; do not leave the rule attached to a build split that may never
exist.

### 3.3 The output-surface ban, enforced by structure we own

§1.3 is clean today, and the property to keep is that it stays clean by
construction rather than by vigilance:

- No `Debug` / `Display` on any type holding the signer, derived or
  hand-written. This is necessary and **not sufficient**: a `{:?}` on a
  tuple, a `Result`, or an `anyhow` context containing the signer
  bypasses our wrappers entirely, and today's safety comes from an
  upstream redacting impl that a dependency bump can remove (§1.3).
  So make it ours: **hold the signer behind our own newtype with no
  formatting impl**, which is the same handle §3.1 already requires. At
  minimum — if the newtype is deferred — add a test that formats the
  signer-bearing type and fails if secret bytes appear, so an upstream
  regression becomes a red build rather than a silent leak.
- Nothing key-shaped in an error message. The existing error path
  interpolates the key *path*, not its contents (`main.rs:174`); that
  distinction is correct and worth preserving.
- Panics and backtraces must not carry it. **Be accurate about the
  mechanism, because the obvious one is wrong:** a Rust backtrace is
  symbol names plus file and line, and never renders argument *values*,
  so passing by reference rather than by value buys nothing here. The
  controls that actually work are the two above — no formatting impl,
  no panic message that interpolates the secret — plus disabling core
  dumps (§3.5), where argument values genuinely are recoverable.
- Nothing key-shaped through telemetry rows, the parameter channel, or
  the TUI.

### 3.4 Least privilege, blast radius, and recovery

Assume the attacker has read the repository and knows the binary, its
configuration shape, and the AWS layout.

**What compromise yields.** The leader signs exactly three instructions
(§3.1), so an attacker holding it can move the maker's quoted reference
price, invalidate it, and reshape its liquidity profile. That is enough
to quote badly and to bleed the vault through adverse fills; it is not
a direct transfer authority, and the leader is not an admin, upgrade,
or mint authority. Naming this is what makes the bound below concrete.

**Note this section and §3.5 assume different adversaries, and keep
them straight.** The metadata bullet below concerns an attacker who
does *not* yet have code execution and is trying to obtain credentials;
everything else here concerns one who does. Both are real; conflating
them is how a control gets credited against a threat it does not
address.

- The execution role must hold **no other secrets-read privilege** and
  must not be able to reach sibling secrets. (It will legitimately need
  logging, telemetry and RPC egress — "one secret and no other
  privilege" is not achievable and should not be written as if it were.)
- Encrypt the secret under a **customer-managed KMS key** whose policy
  names the execution role. Under the account-default
  `aws/secretsmanager` key, decryption is effectively gated on the
  Secrets Manager permission alone, which makes the bullet above a
  weaker property than it reads.
- The signing account stays scoped to the maker's function — not an
  admin, upgrade, or mint authority. Those are separate identities on
  localnet already and that separation must survive the promotion (see
  §4 for the sibling that holds the other half).
- Program-level limits are the real bound, because they hold even when
  the host is fully compromised. **No such cap exists today and none is
  specified here** — what the program should cap, and whether it can,
  is an open question (§7), not a requirement a builder can check
  against.
- **Secret access must be audited, and that audit must raise an
  alert.** Every control
  above is preventive; without detection, a stolen-credential read is
  indistinguishable from normal operation. The mechanism belongs to the
  deploy work — `cloudtrail.yml` already exists, so the substrate is
  there — but the requirement that a compromise be *noticeable* belongs
  here.
- Instance metadata is an exfiltration path for role credentials;
  require IMDSv2. Note this defends against the first adversary above,
  not the second — it buys nothing against code execution on the host.
- **Rotation and revocation.** A Solana keypair cannot be revoked: the
  only remedy after suspected compromise is migrating the on-chain
  authority to a new pubkey. So the signing authority must be
  replaceable **without redeploying the market or draining vaults**,
  and that path must be *exercised* before the first funded key, not
  merely believed to exist. Critically, the authority that can install
  a new leader must be an identity **other than the maker's own
  signer** — the same argument as the kill switch below. Otherwise a
  host compromise does not merely steal signing, it rotates to the
  attacker's key and locks the operator out, turning a recoverable
  incident into a permanent one.
- The kill switch must be reachable by an identity **other** than the
  maker's own signer, or a compromise that holds the host also holds
  the ability to prevent its own shutdown.

### 3.5 Memory hygiene where it is load-bearing

Zeroization is worth doing at the boundary that actually matters — the
transient material holding the fetched secret between the provider
response and the signer — and is largely ritual elsewhere, because the
signer must keep usable key material for the life of the process.

**Do not write this as "zeroize every intermediate", because that rule
cannot be satisfied.** A provider fetch produces several copies: a TLS
buffer, the SDK's own `String`/`Vec`, a serde intermediate, and any
backing store that was reallocated while growing and then freed. Some
of those live inside third-party code and cannot be reached at all. A
rule everyone quietly fails is exactly the theatre this section exists
to avoid. So the honest requirement is: **minimize the number of copies
on the fetch path**, zeroize the ones we own, and state plainly which
intermediates are outside our control rather than implying the boundary
is fully covered.

State the limit too: zeroization does not defend against a live
attacker with code execution. It bounds what a core dump or a swapped
page can contain. So pair it with the two controls that make those
bounds real — **disable core dumps for the process, and run the host
without swap.** Claiming the swapped-page benefit without a swap
control would be the same overreach this section is written against.

### 3.6 CI never sees a production key

No deploy workflow may place a production signing key in a build log,
an artifact, a cached layer, or an image. The deployed process fetches
its own secret at runtime under its own role; CI's job is to ship a
binary that knows how to do that, never to hold the key itself.

§1.5 verified this property for today's workflows. It must be
re-verified for the deploy workflow when one exists — that is a named
item in the follow-up audit, not something this document can settle.

## 4. Two trust domains, kept apart on purpose

There is separate work covering the local secrets enclave — the
1Password-backed material that agent tooling and the FX collectors read
through an operator file. **That is a different trust domain from this
one** and the two must not be merged: the enclave serves developer
sessions on a workstation, while this document serves a production
signing identity that moves real value. They differ in who holds them,
what compromise costs, and what the recovery is. Folding them into one
mechanism would invite exactly the confusion this audit exists to
prevent, so the cross-reference is deliberately just this paragraph.

**Scope note on the sibling binary.** `bots/taker-bot` uses the
identical file-based pattern and holds a mint authority as well as its
taker key. It is **out of scope for this document**, which covers the
maker's signing key only — but the exclusion is worth stating rather
than leaving silent, because §3.4 leans on the maker being separate
from the mint authority as a *control*, and that control is only worth
what the other half's custody is worth. Whether the taker bot is ever
promoted off localnet, and under what custody, is its own question.

## 5. The guard's removal is a milestone, not an alarm

The residual risk accepted today is explicit: **a committed, plain-text
localnet signing key, safe only because the binary refuses the three
public Solana clusters** (§1.1, and note the denylist's exact shape
there). That single guard is doing all of the work.

**But it is not a standing control to watch, and writing it as one
would be a mistake.** §3.2 contemplates a build that can talk to a
public cluster, which by definition does not run the localnet guard. So
the guard's removal is not a surprise to detect — it is a **scheduled
event**, and this document's §3 is the precondition list that must be
in force *before* it happens. Read §3 as the gate on that milestone,
not as advice.

Two things follow. First, the "make the guard a positive allowlist
rather than a denylist" improvement (§1.1) applies to the window before
a mainnet-capable build exists; afterwards there is no guard to
strengthen, only §3 to satisfy. Second, this document is re-opened when
any of the following changes:

- the signing seam — how the key is obtained, held, used to sign, or
  what it is permitted to sign;
- the compute stack the maker runs on, or its execution role;
- any deploy workflow that touches the maker;
- **a bump of `solana-keypair` or `ed25519-dalek`**, which can silently
  remove the upstream redaction §1.3 depends on — unless §3.3's newtype
  has made that moot;
- the localnet guard of §1.1, whose loosening moves the milestone
  forward.

## 6. The recurring key-custody audit

Key and secret custody is reviewed on a recurring basis, each pass
filed as its own board issue. **The cadence lives in the planning
state, not here** — an interval written into a document goes stale the
moment it changes.

**Charter.** Measure and report what is actually found. Expect the
premise to be wrong: a pass that assumes the previous pass's picture
still holds is not a pass. Confirmed negatives are recorded so the next
pass inherits rather than re-measures them — **and every confirmed
negative is inherited with its bound, never as a general clean bill of
health.** §1.3 is the worked example: "the secret reaches only six
signing sites" is a fact about the maker binary's call sites at a
commit, and says nothing about surfaces outside that binary.

**The §1 pass is the template for one category and does not cover the
other.** §1 enumerated *code-emission* surfaces — what the binary can
print, log, or serialize — and measured them clean. It structurally
could not reach *operator-tooling* surfaces: material rendered by
something inspecting the running system rather than emitted by the
program. That distinction is not academic. A raw `docker inspect`
printed a resolved provider API key in plain text (since rotated), on a
day when every code-side negative in this document held. The binary can
be spotless and the key still ends up in a transcript.

Each pass re-checks at least:

1. **Session transcripts and container environment** — agent and
   operator tooling output, `inspect` output, environment dumps, shell
   scrollback. This is the category §1 could not reach, and the one
   with a demonstrated live incident.
1. **CI logs, artifacts and caches**, including any deploy workflow
   (§1.5 covers only today's workflows, and only the non-deploy ones).
1. **Rendered dashboards and stored error columns** — Grafana panels
   and the health-path rows in the store, where a failing request can
   carry a credential into a message that is then persisted.
1. **Telemetry rows and the parameter channel.**
1. **Panic output and backtraces**, per the mechanism in §3.3 rather
   than the intuitive one.
1. **The signing seam itself** — the §1.3 producer set, re-derived, not
   trusted.

## 7. Open questions

- **Process separation versus in-process seam.** A signing sidecar
  isolates the key behind a process boundary at the cost of an IPC
  surface; an in-process seam is simpler and fails open to anything
  with code execution. Not settled here because the answer depends on
  the compute shape, which is not yet designed. Note §3.1 holds either
  way, and that the payload policy matters *more* than the isolation
  choice: a sidecar without one buys key confidentiality and nothing
  else.
- **What the program should cap, and whether it can.** §3.4 calls
  program-level limits the real bound but names none, because none
  exist. Deciding what a compromised leader should be unable to do at
  the program level — and whether the current instruction set admits
  such a cap — is the highest-value open item here, since it is the
  only control besides §3.1's payload policy that survives full host
  compromise.
- **Whether the leader identity should be per-market.** Today one
  leader quotes every market. Distinct signers per market would bound a
  compromise to one book at the cost of more keys to custody. Cheaper
  to decide before the provider is built than after.
- **Whether the localnet guard should be a compile-time feature rather
  than a runtime check.** A runtime check is one edit away from being
  removed; a build that cannot reach a public cluster is a stronger
  guarantee, and §3.2's flag rule depends on which way this goes. Note
  the anchor constraint that individual instructions cannot be compiled
  out governs **on-chain `#[program]` instructions** — it does not apply
  here. The maker bot is a plain binary, so feature-gating both the
  flag and the guard is unobstructed, and this question is open on
  design grounds rather than technical ones.
