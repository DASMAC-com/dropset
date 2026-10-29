# The runtime config, as a placeholder copy — the untracked half of the
# session helpers, kept in the shell profile (`~/.zshrc`) ABOVE the line that
# sources `.claude/shell/init.zsh`. Copy the shape, never commit the values:
# model ids and 1Password coordinates are ruled out of the repo, so every such
# value below is a placeholder. The fleet knobs carry one machine's worked
# values (an 18-core, 48 GiB machine, 2026-10-28); set your own.
#
# Every variable here is a row of `runtime-config.manifest.json`, which says
# its kind, whether it is required, and its repo default. Check a profile with
# `python3 .claude/tools/runtime_config_check.py --live` (it reads only
# exported variables; the morning upkeep pass hands it the rest).
# See docs/conventions/local-integrations.md → "Session helpers".

# --- Model tiers (required; no model id is committed) -----------------------
DS_MODEL_ADVISOR='<model-id>'
DS_MODEL_EXECUTOR='<model-id>'
DS_MODEL_HELPER='<model-id>'
# Optional: Bedrock's background slot. Unset, Bedrock routes to the session's.
DS_MODEL_BACKGROUND='<model-id>'
# Optional substrate overrides (defaults: advisor anthropic, the rest bedrock).
# DS_MODEL_ADVISOR_SUBSTRATE='bedrock'
# DS_MODEL_EXECUTOR_SUBSTRATE='anthropic'
# DS_MODEL_HELPER_SUBSTRATE='anthropic'

# --- 1Password coordinates (pointers, never values) -------------------------
DS_OP_ACCOUNT='<account>.1password.com'
DS_OP_LINEAR_REF='op://<vault>/<item>/<field>'
DS_OP_GITHUB_REF='op://<vault>/<item>/<field>'
DS_OP_BEDROCK_REF='op://<vault>/<item>/<field>'

# --- Linear ids (the filing skills read each with its own printenv) ---------
export LINEAR_TEAM_ID='<team-id>'
export LINEAR_PROJECT_ID='<project-id>'
export LINEAR_ASSIGNEE_ID='<user-id>'
export LINEAR_PLANNING_DOC_ID='<document-id>'

# --- Fleet budget (required; the repo pins none) ----------------------------
# Live top-level `claude` clients of every class at once.
DS_FLEET_TOTAL=16
# The one-minute load average (cores in use) past which nothing new starts.
DS_FLEET_CPU_LOAD_MAX=16
# Builds running at once across the fleet: the cargo pool's lane count.
DS_CARGO_LANES=2
# Compiler processes one build may run. Exported, so cargo reads it everywhere.
# The fleet's compiler ceiling is lanes × jobs: 16 here.
export CARGO_BUILD_JOBS=8

# --- Optional knobs and toggles (each at its repo default when unset) -------
# DS_AWS_PROFILE='<sso-profile>'   # `plan`'s AWS gate; unset = the CLI default
# DS_FLEET_BUILD_WAIT_S=120        # a pool-lock wait refusing another builder
# DS_FLEET_RESERVE_GB=6            # memory headroom; default physical ÷ 8
# DS_BEDROCK_REGION='us-west-2'
# DS_PR_POLL_S=20                  # seconds between every PR / queue poll
# DS_CARGO_POOL=0                  # turn the cargo pool off
# DS_PLAN_TAB_RIGHTMOST=0          # leave the scheduler tab where it opens
# export DS_UPKEEP_ARMED=1         # let the upkeep pass prune and wipe
# Per-command toggles, set for one run rather than in the profile:
# DS_SKIP_UPKEEP=1 plan            # skip the morning upkeep pass
# DS_BEDROCK_PROBE=1 task <n>      # pay for the Bedrock pre-flight
# DS_PULL_DEBUG=1 cdds             # say why a fast-forward did not run
# DS_ITERM_DISABLED=1              # no iTerm tab driving (tests, headless)
