<!-- cspell:word boto -->

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
  bedrock-workers.yml Bedrock worker IAM user, invoke policy, spend alert
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

1. **Bedrock workers — admin, once.** Creates an IAM user, so it needs
   the same named-IAM capability as the baseline. See "Bedrock worker
   identity" below for the two out-of-band steps that follow it.

   ```sh
   aws cloudformation deploy \
     --template-file infra/aws/bedrock-workers.yml \
     --stack-name dropset-dev-bedrock-workers \
     --parameter-overrides file://infra/aws/params/bedrock-workers.dev.json \
     --capabilities CAPABILITY_NAMED_IAM
   ```

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

## Bedrock worker identity

`bedrock-workers.yml` stands up the identity that unattended worker
sessions authenticate as: an IAM user, a managed policy scoped to
invoking two model families, and an optional monthly spend alert.
Operator-attended sessions are unaffected — they keep using the
subscription and never touch this stack.

Two steps cannot be expressed in CloudFormation and follow the deploy
by hand. Both are one-time.

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

**The key is operator-only.** It is generated in the console, copied
once, and pasted into 1Password by a person. No tool, script, or agent
session ever reads, prints, or handles the value — which is why this is
a console procedure rather than a command this repo could run for you.

In the IAM console: **Users** → `dropset-dev-bedrock-worker` →
**Security credentials** → **API keys** → **Generate API key** → choose
**Amazon Bedrock** as the service → pick an expiration (90 days keeps it
on the existing rotation rhythm) → **Generate**, then copy the value.

Two things to know about that dialog:

- The value is shown **once**. There is no way to read it back later, so
  a lost key is re-minted and the old one deactivated, never recovered.
- Generating the key **auto-attaches the `AmazonBedrockLimitedAccess`
  managed policy** to the user. That is broader than this stack intends,
  so detach it afterwards under the user's **Permissions** tab, leaving
  only `dropset-dev-bedrock-worker-invoke`. Claude Code needs nothing
  the invoke policy does not already grant.

**Rotating it.** The key expires on the date chosen at generation, and
expiry is silent from this repo's side — nothing here warns you. List
the current credential, its status and its expiry date with:

```sh
aws iam list-service-specific-credentials \
  --user-name dropset-dev-bedrock-worker \
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
1. Launch a worker session and confirm it makes a real call.
1. Only then deactivate or delete the old credential, by its
   `ServiceSpecificCredentialId`.

**Generating a key auto-attaches `AmazonBedrockLimitedAccess` again**, so
the detach in the previous step is part of *every* rotation, not just the
first. Check the user's Permissions tab afterwards: it should list only
`dropset-dev-bedrock-worker-invoke`. A rotation that skips this silently
re-widens the worker's permissions and leaves the live user out of step
with what this template declares.

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
key exists:

```sh
curl https://bedrock-mantle.us-east-1.api.aws/v1/data_retention \
  -H "x-api-key: $BEDROCK_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{ "mode": "aws_review" }'
```

Models whose `allowed_modes` include `none` are unaffected by the
account setting — a more permissive account mode does not cause their
content to be retained.

### Why the `us.` inference profile, not `global.`

Both profiles exist and are `ACTIVE` for Fable 5.1. They differ in the
foundation-model ARNs they route to, which is what settles it:

| Profile                             | Routes to                             |
| ----------------------------------- | ------------------------------------- |
| `us.anthropic.claude-fable-5-1`     | `us-east-1`, `us-east-2`, `us-west-2` |
| `global.anthropic.claude-fable-5-1` | a region-less ARN, i.e. anywhere      |

The global profile's region-less ARN cannot be pinned in an IAM policy,
so residency could only be asserted, never enforced. The `us.` set is
three named regions, which the invoke policy pins with a `us-*` resource
wildcard. Retention follows the destination region, and this account has
just opted into having that content retained for human review — so
keeping it inside US regions is the conservative pairing. Revisit only
if throughput headroom ever justifies it.

### Launching a worker session by hand

Until the launcher learns this (a later phase), a worker session is
started with these exports:

```sh
export CLAUDE_CODE_USE_BEDROCK=1
export AWS_REGION=us-west-2
export ANTHROPIC_MODEL='us.anthropic.claude-fable-5-1[1m]'
export ENABLE_PROMPT_CACHING_1H=1
export AWS_BEARER_TOKEN_BEDROCK="$(op read \
  --account "$DS_OP_ACCOUNT" "$DS_OP_BEDROCK_REF")"
```

`DS_OP_BEDROCK_REF` is the `op://` coordinate, defined alongside the
other `DS_OP_*` coordinates in the untracked runtime config — the same
split the committed shell helpers already use, where anything tracked
carries placeholder shapes only. Resolving it at launch rather than
exporting the key into a long-lived shell keeps the value out of every
process that does not need it.

Setting `ANTHROPIC_MODEL` does more than pick the primary model: on
Bedrock it also routes background tasks (session titles and the like) to
that same model. That is what keeps the two-model policy sufficient —
left unset, background tasks default to a Sonnet model this policy does
not grant, and they would fail. Add any further model to the template's
parameters before selecting it.

`ENABLE_PROMPT_CACHING_1H` requests the 1-hour cache TTL in place of the
5-minute default, billed at a higher write rate. If cache token counts
stay at zero, the cause is regional cache support rather than this flag.

**The `[1m]` suffix is not decoration.** Fable 5.1 supports a 1M-token
context window, but on a third-party provider the window defaults to
**200k** and the suffix is how you opt in. Claude Code strips it before
calling Bedrock, so it never reaches the provider as part of the model
id — which is also why its absence fails silently rather than erroring:
the session simply runs with a fifth of the context. Confirm it with
`/context`, which prints the window it actually got.

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
