"""The hermetic child environment every zsh-driven suite here spawns under.

Each `test_*.py` that sources the real `init.zsh` used to build its own child
env from `os.environ` minus a hand-kept tuple of names, so isolation drifted
between suites: one isolated `CLAUDE_CONFIG_DIR`, a sibling did not, and when
`plan` gained a transcript-existence check the sibling found the operator's live
transcript, passed locally and failed on CI, which has none. The rule now lives
once: scrub every family a launcher reads operator state from, and point the
transcript tree at a scratch dir the caller owns. A new launcher read of one of
those families therefore cannot pass locally on the operator's state.

Not a `test_*` module, so discovery skips it; the suites import it bare, since
`unittest discover -s .claude/scripts` puts this directory on `sys.path`.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path

#: Every inherited name with one of these prefixes is dropped: the runtime config
#: and helper state (`DS_*`, `_DS_*`), the model pins (`ANTHROPIC_*`,
#: `CLAUDE_CODE_*`), the AWS session and Bedrock token (`AWS_*`), and the
#: 1Password CLI's session and account (`OP_*`).
SCRUBBED_PREFIXES = ("DS_", "_DS_", "ANTHROPIC_", "AWS_", "CLAUDE_CODE_", "OP_")

#: Single names outside those families that change what a launcher does.
SCRUBBED_NAMES = ("CLAUDE_CONFIG_DIR", "ENABLE_PROMPT_CACHING_1H")


def hermetic_env(
    claude_home: str | Path,
    *also_scrub: str,
    env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    """`os.environ` with the operator's state removed, ready for `subprocess`.

    `claude_home` becomes `CLAUDE_CONFIG_DIR`, so no case sees real transcripts;
    it need not exist. `also_scrub` names a suite's own extra variables (the
    cargo pool's `CARGO_*`, say). `env` is applied last and wins over all of it,
    which is how a case sets a `DS_*` knob it means to test. `DS_ITERM_DISABLED`
    is forced on so no case can reach a real iTerm tab (`iterm_api.DISABLED_ENV`).
    """
    drop = set(SCRUBBED_NAMES) | set(also_scrub)
    child = {
        name: value
        for name, value in os.environ.items()
        if name not in drop and not name.startswith(SCRUBBED_PREFIXES)
    }
    child["CLAUDE_CONFIG_DIR"] = str(claude_home)
    child["DS_ITERM_DISABLED"] = "1"
    child.update(env or {})
    return child
