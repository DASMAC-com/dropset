# AWS infrastructure (CloudFormation)

Account foundation for the EURC-liquidity survey and any later AWS
work. These templates stand up the network and identity baseline that
the survey application stack (S3 + RDS + Fargate collectors, authored
separately) attaches to.

Templates are CloudFormation YAML. They are deliberately plain and
parameterized — no CDK, no hard-coded account ids, regions, or CIDRs.

## Layout

```text
infra/aws/
  network.yml         VPC, public/private subnets (2 AZs), NAT, routing
  iam-baseline.yml    CFN deployment role, agent role, secrets policy
  cloudtrail.yml      multi-region audit trail + private log bucket
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

To let a *restricted* identity provision stacks that do create IAM (the
survey app stack's task roles), pass the deployment role so
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
