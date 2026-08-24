<!-- cspell:word dalek -->

<!-- cspell:word IMDS -->

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
lands on; the citations in §1 are line-anchored and should be
re-derived, not trusted, on a later read. The spec half is a standing
posture, not a one-shot — §6 names the trigger that re-opens it.

## 1. Current state: the signing key today

### 1.1 The maker cannot run off localnet at all

`bots/maker-bot/src/main.rs` guards before it loads anything:

```rust
chain::assert_localnet(&client)?;
let leader = solana_keypair::read_keypair_file(&args.leader_key)
```

The guard runs *before* the key is read, not after. So there is
currently no mainnet signing path in the maker — the binary refuses to
start against a public cluster. Every risk below is therefore latent
rather than live, and the deployment work is what makes it live.

This ordering is load-bearing and should stay that way: a guard placed
after the key load would already have the secret in memory before
deciding it was misconfigured.

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

| Form                   | Sites                   | Reaches                      |
| ---------------------- | ----------------------- | ---------------------------- |
| `&ctx.leader` (secret) | 6, all in `tasks.rs`    | `chain::` signing calls only |
| `ctx.leader.pubkey()`  | 2, plus startup logging | telemetry and state reads    |

The result is a confirmed negative on the leak question: **the secret
half never reaches telemetry, the parameter channel, the TUI, or any
log line.** Only the public key does. Startup output prints
`leader.pubkey()` when funding, and the fill subscription is keyed on
`leader.pubkey()`; neither touches the secret.

Two structural facts support that:

- `Context` does **not** derive `Debug`. A derived `Debug` on a struct
  holding a `Keypair` is the classic accidental-disclosure path — a
  single `{:?}` in an error branch would dump it. It is absent, and it
  should stay absent. This is worth a comment on the struct, because
  the next person to add a field may reach for `#[derive(Debug)]`
  without knowing what else is in there.
- Signing funnels through one private `send` in `chain.rs`, which is
  the closest thing to a single seam the maker currently has.

### 1.4 The defect: the key is cloned per market

`main.rs` builds one `Context` per market inside a loop, and each gets
its own copy of the leader:

```rust
leader.insecure_clone(),
```

So the 32-byte secret exists in memory once per market, in long-lived
structs, for the life of the process — and the method that puts it
there is named `insecure_clone` by upstream precisely because it
defeats the single-owner discipline the type is designed around.

This directly contradicts the stated goal of one narrow signing seam.
The context doc comment even says the markets "share a leader", which
is true of the *identity* and false of the *storage*.

The fix is a design question rather than a one-line change, so it is
specified in §3.1 rather than patched here: the contexts should share
one signer rather than hold N copies of a secret. On localnet with a
throwaway committed key the present cost is nil; the reason to fix it
is that the deployed shape inherits this structure unless it is changed
first.

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

### 3.1 One signer, one seam

The process must hold exactly **one** owner of the secret, behind a
narrow interface that exposes signing and never exposes bytes. Callers
receive something that can sign; they do not receive a `Keypair`.

Concretely, this retires the per-market `insecure_clone` of §1.4:
contexts share one signer handle rather than each owning a copy. This
bounds every later question — memory hygiene, output surfaces, and
audit scope all get easier when there is one place the secret lives.

### 3.2 The key never lands on disk

The deployed maker must not read its signing key from a file. No
keypair file in the image, in a bind mount, in the compose shape, or
written to a temporary path at startup. The key is fetched from the
secrets provider into memory by the process that uses it.

This also rules out the `--leader-key <path>` flag as the production
input. That flag is a localnet affordance and should be unreachable —
or absent — in a build that can talk to a public cluster.

### 3.3 The output-surface ban stays enforced by structure

§1.3 is clean today by construction, and that is the property to keep:

- No `Debug` / `Display` on any type holding the signer, derived or
  hand-written.
- Nothing key-shaped in an error message. Note the existing error path
  interpolates the key *path*, not its contents — that distinction is
  correct and worth preserving.
- Panics and backtraces must not be able to carry it. A backtrace can
  capture arguments; keeping the secret behind one handle that is never
  passed as a value is what prevents this, which is §3.1 again.
- Nothing key-shaped through telemetry rows, the parameter channel, or
  the TUI. Public keys are fine and already flow there.

### 3.4 Least privilege and a bounded blast radius

Assume the attacker has read the repository, knows the binary, its
configuration shape, and the AWS layout. The question is what
compromise of the instance yields.

- The execution role reads exactly one secret and holds no other
  privilege. It must not be reusable to read sibling secrets.
- The signing account is scoped to the maker's function. It should not
  be an admin, an upgrade authority, or a mint authority — those are
  separate identities on localnet already, and that separation must
  survive the promotion.
- Program-level limits are the real bound, because they hold even when
  the host is fully compromised. Where the program can cap what a
  compromised leader can do, that cap is worth more than any amount of
  host hardening.
- The kill switch must be reachable by an identity **other** than the
  maker's own signer, or a compromise that holds the host also holds
  the ability to prevent its own shutdown.
- Instance metadata is an exfiltration path for role credentials;
  require IMDSv2 and treat metadata reachability as part of the
  deployment's attack surface.

### 3.5 Memory hygiene where it is load-bearing

Zeroization is worth doing at the boundary that actually matters — the
transient buffer holding the fetched secret between the provider
response and the signer — and is largely ritual elsewhere, because the
signer must keep usable key material for the life of the process.

State the honest limit: zeroization does not defend against a live
attacker with code execution on the host. It reduces the window in
which a secret sits in a buffer that outlives its use, and it bounds
what a core dump or a swapped page can contain. Those are real but
narrow benefits, and claiming more of them than that is how this kind
of control turns into theatre.

So: zeroize the transient buffer, do not pretend the resident signer is
protected by it, and prefer disabling core dumps for the process over
asserting that memory is clean.

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
signing identity that moves real value.

They differ in who holds them, what compromise costs, and what the
recovery is. Folding them into one mechanism would invite exactly the
confusion this audit exists to prevent, so the cross-reference is
deliberately just this paragraph.

## 5. Verification and the re-audit trigger

This document is a standing posture. It is re-opened when any of the
following changes:

- the signing seam — how the key is obtained, held, or used to sign;
- the compute stack the maker runs on, or its execution role;
- any deploy workflow that touches the maker;
- the localnet guard of §1.1, whose removal or loosening is what makes
  every latent risk here live.

The residual risk accepted today is explicit: **a committed, plain-text
localnet signing key, safe only because the binary refuses to run off
localnet.** That single guard is doing all of the work, and it is the
thing to watch.

## 6. Open questions

- **Process separation versus in-process seam.** A signing sidecar
  isolates the key behind a process boundary at the cost of an IPC
  surface and more moving parts; an in-process seam is simpler and
  fails open to anything with code execution. This is not settled here
  because the answer depends on the compute shape, which is not yet
  designed. §3.1 holds either way — both need one owner and one seam.
- **Whether the leader identity should be per-market.** Today one
  leader quotes every market. Distinct signers per market would bound a
  compromise to one book at the cost of more keys to custody. Not
  urgent while nothing is deployed, but cheaper to decide before the
  provider is built than after.
- **Whether the localnet guard should be a compile-time feature rather
  than a runtime check.** A runtime check is one edit away from being
  removed; a build that physically cannot reach a public cluster is a
  stronger guarantee. Note the known constraint that individual
  instructions cannot be compiled out under the current anchor setup,
  which is why this is an open question and not a rule.
