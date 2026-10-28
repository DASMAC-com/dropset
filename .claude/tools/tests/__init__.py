"""The tools test package. Importing it switches iTerm automation off for the
whole run (``iterm_api.DISABLED_ENV``), inherited by every subprocess a case
spawns, so no test can open, type into or tint a real tab."""

import os

os.environ["DS_ITERM_DISABLED"] = "1"
