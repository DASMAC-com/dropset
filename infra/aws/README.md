# AWS infrastructure (CloudFormation)

Account foundation for the market-data warehouse and any later AWS
work. These templates stand up the network and identity baseline that
the warehouse stack (S3 + RDS + Fargate collectors, authored separately)
attaches to.

Templates are CloudFormation YAML. They are deliberately plain and
parameterized — no CDK, no hard-coded account ids, regions, or CIDRs.

## Layout

```text
infra/aws/
  network.yml         VPC, public/private subnets (2 AZs), NAT, routing
  iam-baseline.yml    CFN deployment role, agent role, secrets policy
  cloudtrail.yml      multi-region audit trail + private log bucket
  bedrock-agent.yml   Bedrock agent IAM user, invoke policy, spend cap
                      and kill switch, monthly spend alert
  params/             per-stack example parameter files (<stack>.<env>.json)
```

## Conventions

- **Parameterized, not hard-coded.** Environment name, CIDRs, sizes,
  and retention are `Parameters` with sensible defaults. Override per
  environment with a file under `params/`.
- **Exports as the seam.** Each stack exports its outputs (`VpcId`,
  subnet ids, role ARNs, …) namespaced by `EnvironmentName`. Later
  stacks consume them with `Fn::ImportValue` rather than re-declaring
  resources.
- **Linted twice.** Every template is checked by `yamllint` (the
  repo-wide strict config) and by `cfn-lint` (the pre-commit hook,
  scoped to `infra/aws/**`). Keys are ordered alphabetically to satisfy
  `yamllint`; this is cosmetic — CloudFormation is key-order agnostic.
- **No credentials in templates.** Secrets live in Secrets Manager and
  SSM Parameter Store; templates only reference them by ARN.

## Deploy order and identities

`PowerUserAccess` (the day-to-day permission set) can create the
network and audit resources directly, but it intentionally cannot
create IAM roles, and it cannot pass a role to CloudFormation
(`iam:PassRole` is denied). That determines who deploys what.

1. **IAM baseline — admin, once.** Deploy `iam-baseline.yml` from an
   administrator identity (an `AdministratorAccess` permission set). It
   creates the `*-cfn-deployment` role, the MCP-gated
   `*-agent-provisioning` role, and the secrets-read policy.

   ```sh
   aws cloudformation deploy \
     --template-file infra/aws/iam-baseline.yml \
     --stack-name dropset-dev-iam-baseline \
     --parameter-overrides file://infra/aws/params/iam-baseline.dev.json \
     --capabilities CAPABILITY_NAMED_IAM
   ```

1. **Network and audit — PowerUser, directly.** Neither template
   creates IAM resources, so a PowerUser deploys them without
   role-passing.

   ```sh
   aws cloudformation deploy \
     --template-file infra/aws/network.yml \
     --stack-name dropset-dev-network \
     --parameter-overrides file://infra/aws/params/network.dev.json

   aws cloudformation deploy \
     --template-file infra/aws/cloudtrail.yml \
     --stack-name dropset-dev-cloudtrail \
     --parameter-overrides file://infra/aws/params/cloudtrail.dev.json
   ```

1. **Bedrock agent — admin, once.** Creates an IAM user (and the spend
   cap's Lambda role), so it needs
   the same named-IAM capability as the baseline. See "Bedrock agent
   identity" below for the two out-of-band steps that follow it.

   ```sh
   aws cloudformation deploy \
     --template-file infra/aws/bedrock-agent.yml \
     --stack-name dropset-bedrock-agent \
     --parameter-overrides file://infra/aws/params/bedrock-agent.dev.json \
     --capabilities CAPABILITY_NAMED_IAM
   ```

   **Migrating from the old `dropset-dev-bedrock-workers` stack.** The
   stack, the IAM user, the managed policy and all three exports were
   renamed together when the identity noun became *agent*. Because the
   **stack name** itself changed, the new stack is created rather than
   updated — but **create the new one first and delete the old one
   last**, which is safe here and gives the migration no downtime at
   all:

   1. Redeploy `iam-baseline.yml` (above). The agent-provisioning role
      scopes its stack mutations by name, and `dropset-bedrock-agent`
      carries no environment segment, so it must be named there before
      an agent-driven deploy of it can work.
   1. Deploy `dropset-bedrock-agent` with the command above.
   1. Mint the key against the **new** user and confirm a real Bedrock
      call (see "Bedrock agent identity" below).
   1. Delete the **old** user's API key. This step is not optional and
      not deferrable: an out-of-band service-specific credential blocks
      `iam:DeleteUser` with `DeleteConflict`, CloudFormation does not
      know it exists, and the stack delete below fails **part-way**
      without it. Note the user name here is the **old** one:

   ```sh
   aws iam list-service-specific-credentials \
     --user-name dropset-dev-bedrock-worker \
     --service-name bedrock.amazonaws.com
   ```

   ```sh
   aws iam delete-service-specific-credential \
     --user-name dropset-dev-bedrock-worker \
     --service-specific-credential-id <id>
   ```

   1. Only then retire the old stack:

   ```sh
   aws cloudformation delete-stack --stack-name dropset-dev-bedrock-workers
   ```

   **Nothing collides, which is what makes that ordering available.**
   Every name moved in the same deploy — user
   `dropset-dev-bedrock-worker` → `dropset-bedrock-agent`, policy
   `dropset-dev-bedrock-invoke` → `dropset-bedrock-invoke`, and all
   three exports — so the two stacks can coexist, and the old key keeps
   working right up until its user is deleted. Deleting first would
   instead open a window with no Bedrock identity at all, and no
   rollback if the mint then failed.

   The old API key does not survive the old user, so a new one is minted
   against the new user either way: the mint is part of this migration
   and not merely of the first install. Note the credential does not die
   *automatically* — the step above is what removes it, and until it
   does the user cannot be deleted at all. Nothing imports the old
   exports — verified before the rename — which is what made it free to
   take now rather than later.

To let a *restricted* identity provision stacks that do create IAM (the
warehouse stack's task roles), pass the deployment role so
CloudFormation — not the caller — holds the permissions, with
`--role-arn`. The passing identity needs `iam:PassRole` on that role;
the `*-agent-provisioning` role grants it (gated to the MCP server),
whereas `PowerUserAccess` alone does not.

Validate a template without deploying:

```sh
cfn-lint infra/aws/network.yml
aws cloudformation validate-template \
  --template-body file://infra/aws/network.yml
```

## Tearing down

Most stacks delete cleanly and recreate from the templates:

```sh
aws cloudformation delete-stack --stack-name dropset-dev-network
aws cloudformation delete-stack --stack-name dropset-dev-cloudtrail
```

**Two stacks have a resource CloudFormation will not clean up for you —
and they fail in opposite ways.** The CloudTrail log bucket is retained
on purpose, so that stack's delete **succeeds** and simply leaves the
bucket behind (below). The Bedrock agent user carries an out-of-band API
key that blocks `DeleteUser` with `DeleteConflict`, so that stack's
delete **fails part-way** until the credential is deleted first, per
"Bedrock agent identity" below.

The CloudTrail **log bucket is deliberately kept** when its stack is
deleted: it carries `DeletionPolicy: Retain` so an accidental stack
deletion cannot destroy the audit logs. Its name is deterministic
(`${EnvironmentName}-cloudtrail-${AWS::AccountId}`), so a later redeploy
collides with the retained bucket. A *full* teardown — e.g. to recreate
the trail from scratch — is therefore a deliberate extra step: empty and
delete the retained bucket first, then redeploy.

```sh
aws s3 rb s3://dropset-dev-cloudtrail-ACCOUNT_ID --force
```

Because the one deterministic name is reused each cycle, this never
accumulates orphan buckets; `Retain` only makes the delete explicit
rather than automatic.

## Bedrock agent identity

`bedrock-agent.yml` stands up the identity that Bedrock agent
sessions authenticate as: an IAM user, a managed policy scoped to model
invocation in the US regions, a daily spend cap (see "Daily spend cap"
below), and an optional monthly spend alert. The committed parameter
file sets no alert address: you enter `BudgetAlertEmail` yourself, and
it never reaches the repo. Without it there is no monthly budget, since
a budget with an undeliverable subscriber is a tripwire that silently
never fires, and the spend-cap alarms email nobody — though the cap's
kill switch still works. Operator-attended sessions are unaffected —
they keep using the subscription and never touch this stack.

Two steps cannot be expressed in CloudFormation and follow the deploy
by hand. Both are one-time as deploy steps — but note that step 1's
policy detach recurs on every key rotation, as that step explains.

### 1. Mint the API key (out of band, on purpose)

A long-term Bedrock API key is an IAM **service-specific credential**
for `bedrock.amazonaws.com`, and there is no CloudFormation resource
type for one — `AWS::IAM::ServiceLinkedRole` is the only related type
CFN exposes. That absence is convenient rather than limiting: a custom
resource that minted the key would have to surface the secret through
stack outputs or events, which is strictly worse custody than never
letting CloudFormation see it at all.

So the template creates the *user*, and the key is minted against that
user in the IAM console, by hand.

**The cost of that split shows up at teardown, not at create.** A
service-specific credential is a child of the user that CloudFormation
does not know exists, and `iam:DeleteUser` refuses with `DeleteConflict`
while one is attached — so deleting or replacing this stack fails
part-way unless the credential is removed first. Measured during the
worker-to-agent migration. Do this before any delete or rename of the
stack:

```sh
aws iam list-service-specific-credentials \
  --user-name dropset-bedrock-agent \
  --service-name bedrock.amazonaws.com
```

```sh
aws iam delete-service-specific-credential \
  --user-name dropset-bedrock-agent \
  --service-specific-credential-id <id>
```

**This is not an argument for moving the key into the template**, which
is the natural next thought. It cannot go there — there is no resource
type for it, and a custom resource would have to surface the secret
through stack outputs or events, which is the worse-custody trade the
paragraph above rejects. It is also **not drift**: the template declares
no credential, so nothing has diverged from it. It is an ordering
requirement, and documenting it is the whole fix.

**The key is operator-only.** It is generated in the console, copied
once, and pasted into 1Password by a person. No tool, script, or agent
session ever reads, prints, or handles the value — which is why this is
a console procedure rather than a command this repo could run for you.

In the IAM console: **Users** → `dropset-bedrock-agent` →
**Security credentials** → **API keys** → **Generate API key** → choose
**Amazon Bedrock** as the service → pick an expiration → **Generate**,
then copy the value.

**Rotation is monthly**, so the expiry chosen here is a backstop rather
than the schedule — the operator re-mints on the monthly rhythm well
before any of the offered expirations lands. Leak exposure on this key
is bounded to credit burn rather than data, which is what keeps custody
this simple; the cadence is the compensating control.

Two things to know about that dialog:

- The value is shown **once**. There is no way to read it back later, so
  a lost key is re-minted and the old one deactivated, never recovered.

- Generating the key is **reported** to auto-attach the
  `AmazonBedrockLimitedAccess` managed policy to the user. That would be
  broader than this stack intends, so check the user's **Permissions**
  tab afterwards and detach anything beyond
  `dropset-bedrock-invoke` — Claude Code needs nothing the invoke policy
  does not already grant.

  **Treat that as a check, not as a known fact.** The CloudTrail record
  does not corroborate it, across **two** mints:

  - **2026-09-04 00:37:16 UTC** (the first). Over the 90-day window
    there is **no `AttachUserPolicy` for `AmazonBedrockLimitedAccess` at
    all**, and no attach of any kind follows the mint. The only two
    attaches are the template's own — one 36 seconds after `CreateUser`,
    one in the later policy rename.
  - **2026-09-09 00:34:09 UTC** (the worker-to-agent re-mint), and this
    one is a **true before/after on the same user**: attached policies
    were `{dropset-bedrock-invoke}` immediately before the mint and
    `{dropset-bedrock-invoke}` immediately after. The only attach in
    that window is CloudFormation's, 2.5 minutes *before* the mint; the
    only detach is the old stack's teardown, 12 minutes after.

  Read the bound on that honestly before acting on it. Two mints is
  still a small n, and an absent event is weaker evidence than a present
  one, since the console could in principle attach through an API that
  logs under another name or under an AWS-internal principal this trail
  does not capture. What both windows establish is that management
  events *were* being recorded throughout — `CreateUser`, `CreatePolicy`,
  `CreateServiceSpecificCredential` and both attach/detach pairs are all
  present — so a silent trail is not the explanation.

  Hence: keep the step, drop the certainty. Detaching a policy that was
  never attached costs one glance at a tab; skipping a check that turns
  out to be needed silently re-widens the identity.

  **The lookups must target `us-east-1`.** IAM is a global service and
  its events land there, not in the stack's `us-west-2` — the same query
  against `us-west-2` returns zero events for every one of these names
  and reads exactly like "it never happened".

**Rotating it.** Rotate monthly. The key also expires on the date chosen
at generation, and expiry is silent from this repo's side — nothing here
warns you, and the launcher's only job is to make the first failed call
legible rather than opaque. List the current credential, its status and
its expiry date with:

```sh
aws iam list-service-specific-credentials \
  --user-name dropset-bedrock-agent \
  --service-name bedrock.amazonaws.com
```

That returns metadata only — never the secret — including the
`ServiceSpecificCredentialId` the delete step needs.

Rotate by generating *before* revoking, so there is no window with no
working key:

1. Generate a new key the same way, in the IAM console.
1. Update the existing 1Password field in place. Launching resolves the
   reference fresh each time, so nothing else has to change: no redeploy,
   no edit to this repo, no change to the runtime config.
1. Launch an agent session and confirm it makes a real call.
1. Only then deactivate or delete the old credential, by its
   `ServiceSpecificCredentialId`.

**Re-check the attached policies after every mint**, not just the first
— the check above is part of every rotation. The user's Permissions tab
should list only `dropset-bedrock-invoke` — plus, after a spend-cap
trip, `dropset-bedrock-spend-cap-deny` (see "Daily spend cap" below).
That one is the expected exception: it only removes access, and its
`AttachUserPolicy` event in CloudTrail is made by an assumed role whose
generated name contains `SpendKillFunctionRole`, not by a person. A
rotation that skips the check would leave a re-widened identity out of
step with what this template declares, and nothing else would report it.

Verify it from the command line with the same two reads, remembering the
region:

```sh
aws iam list-attached-user-policies --user-name dropset-bedrock-agent
```

```sh
aws cloudtrail lookup-events --region us-east-1 \
  --lookup-attributes AttributeKey=EventName,AttributeValue=AttachUserPolicy
```

**The first needs an IAM-capable identity; the second does not.**
`cloudtrail:LookupEvents` is not an IAM action, so `PowerUserAccess` can
run the lookup — but it is denied `iam:ListAttachedUserPolicies` and
`iam:ListServiceSpecificCredentials` outright, so an agent session on
the usual SSO role cannot run the direct permissions read at all. On
that role, CloudTrail is the only one of the two checks available.

Store it in 1Password as one item per provider with a named field per
credential, giving a reference of the shape
`op://<vault>/<item>/credential` — the same shape the other session
secrets use, and a valid Secrets Manager id under the `dropset/` prefix
if one is ever needed there (see
`infra/localnet/secrets.local.env.example`). Only the placeholder shape
belongs in tracked files: the real vault and item names stay in the
untracked runtime config, because committing them would publish the
layout of a personal secret store into permanent git history.

### 2. Opt the account into `aws_review` retention

**Done on 2026-09-03**, in all three routed regions; recorded here
because it is invisible in the console. Claude Fable 5 and 5.1 require
human review as a condition of access, so a region left at the default
`inherit` mode resolves to `default` and blocks every request to them.

**The setting is PER-REGION, despite being called account-wide, and this
is the trap.** `PutAccountDataRetention` writes only the region it is
called in. Retention follows the *destination* region, and the `us.`
inference profile routes across three of them, so all three need it —
setting only one leaves the others at `inherit`, and a request that
routes to a missed region fails with

```text
400 data retention mode 'default' is not available for this model
```

which names neither a region nor the setting, and looks nothing like a
retention problem. This was hit for real: the opt-in was made in
us-east-1 while inference ran in us-west-2. Set it in every region the
chosen profile routes to, and read each one back.

Review is carried out **by AWS, inside the AWS boundary**. Content is
not shared with the model provider — `provider_data_share` is a legacy
mode that grants a permission AWS does not exercise today, and new
configurations use `aws_review`.

There is no console UI for this, and no `aws bedrock` CLI subcommand
either — it is an API-only setting. It was set here through the Bedrock
control-plane operations `GetAccountDataRetention` and
`PutAccountDataRetention` (`GET` and `PUT /data-retention`, body
`{"mode": "aws_review"}`) signed with SigV4, which is what let it be
done before any API key existed. Any SigV4 client works; with `boto3`:

```python
for region in ('us-east-1', 'us-east-2', 'us-west-2'):
    boto3.client('bedrock', region_name=region).put_account_data_retention(
        mode='aws_review')
```

Read each region back with `get_account_data_retention` rather than
trusting the write: a region still reporting `inherit` is the one that
will fail, and it fails only when a request happens to route there.

The user guide documents an equivalent bearer-token form, useful once a
key exists. It is quoted verbatim below — including its single hardcoded
region, which is exactly the trap above: run it once per routed region,
not once. Note also that the user guide's example omits `-X PUT`, so as
written `curl` sends POST, while the API reference documents the
operation as `PUT /data-retention`; the `boto3` form above is the one
this account was actually configured with.

```sh
curl https://bedrock-mantle.us-east-1.api.aws/v1/data_retention \
  -H "x-api-key: $BEDROCK_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{ "mode": "aws_review" }'
```

Models whose `allowed_modes` include `none` are unaffected by the
account setting — a more permissive account mode does not cause their
content to be retained, which is what keeps executor-tier traffic out of
review while its model allows `none`. The opt-in dates from when
Fable 5.1 was the ratified agent model, and it is kept because a
Fable-class model has to keep working without an infrastructure
change — `plan bedrock`, the advisor tier's credit-pinch override,
depends on it. So read the opt-in as removing a
constraint on which models are selectable, not as a statement about
what happens to executor-tier traffic.

**It cannot be narrowed to spare Fable traffic.** The opt-in is a
condition of Fable access, not a dial on it: a region whose mode is
not `aws_review` refuses Fable outright (the `400` above), and the
mode is per-region rather than per-model or per-identity. So a Bedrock
Fable session is a retained session, and the operator has accepted that
for the pinch case.

### Why the `us.` inference profile, not `global.`

Both profiles exist and are `ACTIVE` for Fable 5.1. They differ in the
foundation-model ARNs they route to, which is what settles it:

| Profile                             | Routes to                             |
| ----------------------------------- | ------------------------------------- |
| `us.anthropic.claude-fable-5-1`     | `us-east-1`, `us-east-2`, `us-west-2` |
| `global.anthropic.claude-fable-5-1` | a region-less ARN, i.e. anywhere      |

The global profile's region-less ARN cannot be pinned in an IAM policy,
so residency could only be asserted, never enforced. The `us.` set is
three named regions, which the invoke policy approximates with a `us-*`
resource wildcard — a prefix match, so it also admits `us-west-1` and
any future `us-` region. That is wider than the set opted into
retention, and still US-only, which is the property at issue
here. Retention follows the destination region, and this account has
just opted into having that content retained for human review — so
keeping it inside US regions is the conservative pairing. Revisit only
if throughput headroom ever justifies it.

### Launching an agent session

**The `task` verb does this for you** — `.claude/shell/init.zsh` exports
the set below, resolves the key from 1Password at launch, and hands the
model to `claude` for that one command rather than exporting it. See
`docs/conventions/local-integrations.md` → "Session helpers" for the
verb table and the substrate rule that decides which verbs get these
exports at all. What follows is the equivalent by hand, for debugging a
launch that misbehaves:

```sh
export CLAUDE_CODE_USE_BEDROCK=1
export AWS_REGION=us-west-2
export ANTHROPIC_DEFAULT_HAIKU_MODEL="$DS_MODEL_BACKGROUND"  # if set
export ENABLE_PROMPT_CACHING_1H=1
export AWS_BEARER_TOKEN_BEDROCK="$(op read \
  --account "$DS_OP_ACCOUNT" "$DS_OP_BEDROCK_REF")"
ANTHROPIC_MODEL="$DS_MODEL_EXECUTOR" claude
```

`DS_OP_BEDROCK_REF` is the `op://` coordinate, defined alongside the
other `DS_OP_*` coordinates in the untracked runtime config — the same
split the committed shell helpers already use, where anything tracked
carries placeholder shapes only. Resolving it at launch rather than
exporting the key into a long-lived shell keeps the value out of every
process that does not need it.

**The model string is runtime config, not code, and not a stack
value.** The launcher reads a role-named tier from that same untracked
file (`DS_MODEL_EXECUTOR` for `task`; the full table is in the
local-integrations convention) and uses it verbatim, window suffix
included; **no model id is pinned anywhere in the repo**, so a new
model is a runtime-config edit only. A first-party id
(`claude-<family>-<version>`) is portable: Claude Code maps it to the
`us.anthropic.` profile on Bedrock (measured), so the same string
serves both substrates. Give it a `[1m]` suffix: Bedrock defaults an
unsuffixed id to the 200k window and nothing reports it, so a string
without one draws a launch-time warning. This stack used to publish a
default model id as an export; nothing read it, so it was retired —
the identity here is model-agnostic.

`ANTHROPIC_DEFAULT_HAIKU_MODEL` pins the background tier
(`DS_MODEL_BACKGROUND`) so background sub-turns bill to credits
alongside the primary model, rather than falling back to the
subscription. Claude Code maps a first-party id in this slot to its
`us.anthropic.` profile, as it does the primary model's (measured from
its request log), so a first-party id is portable; a
Bedrock-form id must be the exact profile id, since Bedrock rejects any
other spelling as an invalid model identifier. A
`ResourceNotFoundException` saying model use case details have not been
submitted means the account has not filed Anthropic's use-case form for
that model — an operator step.

Setting `ANTHROPIC_MODEL` does more than pick the primary model: on
Bedrock it also routes background tasks (session titles and the like) to
that same model. That matters for cost attribution, not for permissions
— the invoke policy grants any foundation model in the US regions, so
nothing fails for want of a grant. The Sonnet auto-mode classifier is
the case in point: Claude Code invokes it regardless of the model
selected here, and the policy covers it. Switching models needs no
template edit and no redeploy.

`ENABLE_PROMPT_CACHING_1H` requests the 1-hour cache TTL in place of the
5-minute default, billed at a higher write rate. If cache token counts
stay at zero, the cause is regional cache support rather than this flag.

**Enabling a model the account has never used.** Serverless foundation
models activate on first invocation, but a model served through AWS
Marketplace additionally needs one invocation by a principal holding
`aws-marketplace:Subscribe` and `ViewSubscriptions` — which the agent
deliberately does not have. So a model new to the account is enabled by
invoking it once as an administrator; afterwards every principal can use
it. The console's model-access page has been retired and no longer does
this.

The first such call fails with an `AccessDeniedException` naming the
Marketplace actions, even for an administrator who demonstrably holds
them. That first call is what starts the subscription; retrying a few
minutes later succeeds. Treat the initial denial as expected rather than
as a permissions fault, and confirm with
`get-foundation-model-availability`, whose `agreementAvailability` flips
from `NOT_AVAILABLE` to `AVAILABLE`.

**The `[1m]` suffix is not decoration.** Opus 5.5 supports a 1M-token
context window, but on a third-party provider the window defaults to
**200k** and the suffix is how you opt in. Claude Code strips it before
calling Bedrock, so it never reaches the provider as part of the model
id — which is also why its absence fails silently rather than erroring:
the session simply runs with a fifth of the context. Confirm it with
`/context`, which prints the window it actually got.

### Daily spend cap

A runaway agent loop must not burn unbounded credits overnight. The
stack estimates Bedrock spend over a **rolling 24 hours** from
CloudWatch's Bedrock token metrics — input, output, cache read and
cache write, summed across every model — priced at the `TokenRate*`
parameters (Opus 5.5 rates). Pricing every model at the dearest model's
rates makes the estimate an upper bound on the charge — but **only
while the rates belong to the dearest model actually running**. On
2026-10-06, leftover Opus 5 traffic priced at Opus 5.5 rates estimated
$162 against $210 billed. Keep the rates on the priciest model in use.

| Estimate vs. `SpendCapUsd` (default 2000) | What happens                                        |
| ----------------------------------------- | --------------------------------------------------- |
| 25%, 50%, 75%                             | Email via the `dropset-bedrock-spend-alerts` topic  |
| 100%                                      | Email, and the deny policy is attached (when armed) |

The trigger is CloudWatch rather than AWS Budgets because billing data
refreshes only about once a day, too late against an overnight loop.
**The estimate is not the bill**, and nothing reconciles the two
automatically yet: compare it against Cost Explorer by hand until a
reconciliation step exists. Two scope limits follow from the metrics:

- They are **per region**, so the cap counts only invocations made from
  the stack's region (us-west-2, the session launcher's default). A
  session pointed elsewhere through `DS_BEDROCK_REGION` spends
  uncounted, although a trip still blocks it, since the deny is global.
- They carry **no identity**, so any Bedrock use in that account and
  region counts toward the cap, not only the agent user's. That errs on
  the safe side.

**Entering the address.** Set `BudgetAlertEmail` once, in the console
(the stack's *Update* → *Use current template* → parameters), or with a
deploy that passes `--parameter-overrides BudgetAlertEmail=<address>`
**instead of** the `file://` override — typed at the prompt, never
committed. Every parameter left out of an override keeps its previous
value (`aws cloudformation deploy` sends `UsePreviousValue` for it), and
the committed parameter file omits this key, so a later routine deploy
keeps the address. AWS then mails a subscription confirmation. **Click
it**, because until you do, the topic delivers nothing. Confirmation
only catches future crossings; an alarm already in ALARM stays silent
until its next transition.

**Resuming after a trip.** When the 100% alarm fires, a small Lambda
attaches `dropset-bedrock-spend-cap-deny` to `dropset-bedrock-agent`,
and every agent call fails with an access-denied error. The alarm email
is not proof the block landed — a failed attach is retried twice and
then dropped silently — so confirm with
`aws iam list-attached-user-policies --user-name dropset-bedrock-agent`.

Look at what tripped it before anything else. Right after a trip the
rolling window still holds the spend that tripped it, so the estimate is
**still over the cap for up to 24 hours**. That decides how to resume:

1. **To wait it out**, do nothing; agents stay blocked. Once the
   estimate has dropped back under the cap, detach and re-arm (both
   commands below).

1. **To resume now, while still over the cap**, raise the cap
   (`SpendCapUsd`) or disarm (below) first, then detach. Detaching
   without one of those is not a resume: re-arming re-trips within about
   a minute, and *not* re-arming leaves the cap off (next point).

Detach:

```sh
aws iam detach-user-policy \
  --user-name dropset-bedrock-agent \
  --policy-arn arn:aws:iam::<account-id>:policy/dropset-bedrock-spend-cap-deny
```

Re-arm:

```sh
aws cloudwatch set-alarm-state --alarm-name dropset-bedrock-spend-cap \
  --state-value OK --state-reason 'Re-armed after a manual resume'
```

**A detach without the re-arm leaves the cap OFF.** The alarm is still
in ALARM, and alarm actions fire only on a transition into ALARM. So
while it stays there nothing re-blocks and no alarm emails, and at a
steady rate at or above the cap it never leaves. The re-arm forces the
transition: the next evaluation, within about a minute, re-trips if the
estimate is over the cap and otherwise stays armed.

**Disarming.** Set `SpendKillSwitchEnabled` to `false` **in the
committed parameter file** and redeploy. A one-off console or CLI
override works too, but the next routine deploy from the committed file
re-arms it — the same holds for `SpendCapUsd`. All four alarms keep
emailing, but the cap blocks nothing. A deny policy that is already
attached stays attached; detach it as above. Re-arming by setting the
value back to `true` takes effect only on the alarm's next transition,
so once the estimate is under the cap, follow it with the re-arm call
above. Before deleting the stack, or changing
`EnvironmentName` (which renames the policy), detach the policy by hand,
since CloudFormation cannot delete an attached managed policy.

**Re-seeding the rates.** When the default model changes, update the
four `TokenRate*` values in the parameter file to that model's Bedrock
rates (Cost Explorer's cost ÷ usage quantity per usage type gives them
exactly) and redeploy.

**Running cost** is at most $1.60 a month: CloudWatch bills a
standard alarm $0.10 per month for each metric its expression lists,
and four alarms × four metrics is 16. The free tier covers 10 alarm
metrics, which brings it to \$0.60 if no other alarm uses them. The token
metrics are free, and the Lambda runs only on a trip.

## Secrets

Application secrets (database passwords, API keys) go in Secrets
Manager under the `${EnvironmentName}/` prefix; non-secret configuration
goes in SSM Parameter Store under the same prefix. Service roles attach
the `*-secrets-read` managed policy (from `iam-baseline.yml`) to read
only their environment's entries. No secret value is ever committed to
a template or a parameter file.

## Agent-assisted authoring

CloudFormation authoring, deployment, and troubleshooting here are
agent-assisted through the AWS MCP Server and the Agent Toolkit for
AWS. The rules an agent follows — prefer the MCP server, discover
skills and search the AWS docs before acting, and keep to least
privilege — are documented in `docs/conventions/aws-infra.md`, along
with the local (not committed) MCP setup.
