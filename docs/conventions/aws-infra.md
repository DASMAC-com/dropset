<!-- cspell:word awslabs -->

# AWS infrastructure and the Agent Toolkit

AWS resources are defined as **CloudFormation YAML** under `infra/aws/`.
The account foundation (network, IAM, audit) lives there; the survey
application stack is authored on top of it. The template layout, the
deploy order, and the secrets pattern are documented in
`infra/aws/README.md` — this file covers the two things that are
project *conventions* rather than per-template detail: how CloudFormation
conforms to the repo's linters, and the rules for the agent-assisted
authoring workflow.

## Linting: yamllint **and** cfn-lint

Templates are checked by both linters, and both gate the PR:

- **cfn-lint** (`infra/aws/**/*.yml`, wired in `cfg/pre-commit-lint.yml`)
  is the CloudFormation-aware linter — it catches bad resource
  properties, missing `Ref` targets, and IAM mistakes.
- **yamllint** (the repo-wide strict config, `cfg/yamllint.yml`) still
  applies house style to the same files. It is unusually strict, so
  CloudFormation must be written to fit it:
  - **Keys are ordered alphabetically** — every mapping, including
    `Resources` logical ids and the keys within a resource (so
    `Properties` precedes `Type`). CloudFormation is key-order
    agnostic, so this is purely cosmetic.
  - **Strings are single-quoted**; use the `!Ref` / `!GetAtt` / `!Sub`
    short tags, whose scalar arguments yamllint does not quote-check.
  - **Block style only** — no flow `{}` / `[]`. Intrinsics that take a
    list (`!Select`, `!If`, `!Equals`) are written as block sequences.
  - **No blank lines**, and every line stays within 80 columns. A long
    ARN exceeds 80 as a `!Sub` scalar; write it as a folded block
    scalar (`!Sub >-` then the ARN on the next line) so the overflowing
    line is a single unbroken word, which yamllint allows.

Validate locally before committing:

```sh
cfn-lint infra/aws/network.yml
yamllint -c cfg/yamllint.yml infra/aws/network.yml
```

## Agent Toolkit for AWS

CloudFormation authoring, deployment, and troubleshooting are
agent-assisted through **two** MCP servers with a deliberate division of
labour — documentation reads need no credentials, so they don't go
through the credentialed server. The rules:

- **Documentation lookups → the credential-free `aws-docs` server.**
  This is the dedicated read-only AWS Documentation MCP server (the
  canonical "just docs" server from the [awslabs/mcp][awslabs] catalog).
  It runs local over stdio, needs **no AWS credentials**, and exposes
  `search_documentation`, `read_documentation`, and `recommend`. Use it
  for all doc reads — not `aws___search_documentation` on the SigV4
  proxy, which couples credential-free reads to a heavyweight,
  IAM-authenticated server that has proven flaky to reconnect. (The
  remote **AWS Knowledge MCP server**,
  `https://knowledge-mcp.global.api.aws/mcp`, is a zero-install,
  also-credential-free alternative with a broader corpus; the dedicated
  Documentation server is the canonical pick.)
- **Account actions → the SigV4 `aws-mcp` server.** The credentialed
  remote server (the managed [Agent Toolkit for AWS][toolkit] proxy) is
  scoped to actual account work — deploy, inspect, CLI execution, and
  skill retrieval (`aws___retrieve_skill`) — where the IAM auth and
  least-privilege roles below actually matter. Do not vendor the
  toolkit's skills into the repo — the server retrieves them on demand.
- **Discover before acting.** Search the AWS docs (via `aws-docs`) and
  retrieve the relevant skill *before* writing a template or running a
  command, so the work follows current AWS guidance rather than memory.
- **Least privilege.** Agent-driven provisioning uses the dedicated
  `*-agent-provisioning` role (`infra/aws/iam-baseline.yml`), whose
  permissions activate only when the request flows through the MCP
  server — enforced with the `aws:ViaAWSMCPService` and
  `aws:CalledViaAWSMCP` condition keys. Deploys pass the
  `*-cfn-deployment` role rather than relying on the caller holding
  IAM-write. CloudTrail (`infra/aws/cloudtrail.yml`) records the
  resulting activity for audit.

## Local setup (not committed)

Both MCP servers are wired into **user-local** settings (`--scope user`),
never the repo — the same not-committed-settings convention as
[the GitHub MCP](github-mcp.md) and
[local integrations](local-integrations.md). Nothing here belongs in a
tracked file.

The **`aws-docs`** documentation server is credential-free and runs
local over stdio — no `aws login`, no signing:

```sh
claude mcp add aws-docs --scope user -- \
  uvx awslabs.aws-documentation-mcp-server@latest
```

The **`aws-mcp`** account-action server authenticates with **SigV4**
(the mode the Agent Toolkit recommends for terminal coding agents),
which reuses the AWS CLI credentials the templates are deployed with:

```sh
aws login
claude mcp add aws-mcp --scope user -- uvx mcp-proxy-for-aws==1.6.3 \
  https://aws-mcp.us-east-1.api.aws/mcp --metadata AWS_REGION=us-west-2
```

`aws login` writes short-lived, auto-rotating credentials; the proxy
signs each MCP request with them. As with the other user-scope MCP
servers, a newly added server loads on the **next** Claude Code
session, not mid-session. Credentials are never committed: no access
keys in the repo, no secrets in templates (see `infra/aws/README.md`).

[awslabs]: https://github.com/awslabs/mcp
[toolkit]: https://docs.aws.amazon.com/agent-toolkit/
