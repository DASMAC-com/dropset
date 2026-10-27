#!/usr/bin/env python3
"""Unit tests for spec_markers.py, over embedded spec fixtures.

The measured failure is an index-only draft: markers listed at the top and
absent where the input is wanted. Each failure case below is one way the index
and the inline markers can disagree; the clear cases pin down that a correct
spec, and one with no markers at all, pass.
"""

from __future__ import annotations

import io
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

import spec_markers as sm

CLEAR = """# 2217 curator

## Open items

- §2 Pricing — NEEDS-CONFIRM fair = FX × basis
- NEEDS-FEEDBACK the [rollout order](#rollout-order)
- NEEDS-CONFIRM drop the stale-quote tier entirely

## 1. Context

Settled text carries no marker.

## 2. Pricing

NEEDS-CONFIRM: fair = FX × basis, with basis from the venue mid.

## Rollout order

NEEDS-FEEDBACK: which market goes first?

## Tiers

We propose to drop the stale-quote tier entirely. NEEDS-CONFIRM

```text
NEEDS-CONFIRM inside a fence is an example, not an open item
```
"""

INDEX_ONLY = """# 2217 curator

## Open items

- NEEDS-CONFIRM §2 Pricing
- NEEDS-FEEDBACK §3 Rollout

## 2. Pricing

Fair = FX × basis.

## 3. Rollout

AUD first.
"""

UNLISTED = """# Spec

## Open items

- NEEDS-CONFIRM §1 Pricing

## 1. Pricing

NEEDS-CONFIRM: fair = FX × basis.

## 2. Rollout

NEEDS-FEEDBACK: which market goes first?
"""

NO_INDEX = """# Spec

## 1. Pricing

NEEDS-CONFIRM: fair = FX × basis.
"""

SAYS_NONE = """# Spec

## Open items

- None.

## 1. Pricing

NEEDS-CONFIRM: fair = FX × basis.
"""

PREAMBLE_LIST = """# Spec

- NEEDS-CONFIRM §1 Pricing

## 1. Pricing

NEEDS-CONFIRM: fair = FX × basis.
"""

KIND_MISMATCH = """# Spec

## Open items

- NEEDS-FEEDBACK §1 Pricing

## 1. Pricing

NEEDS-CONFIRM: fair = FX × basis.
"""


class CheckTests(unittest.TestCase):
    def test_clear_spec_agrees(self):
        count, failures = sm.check(CLEAR)
        self.assertEqual(failures, [])
        self.assertEqual(count, 3)

    def test_no_markers_is_clear(self):
        self.assertEqual(sm.check("# Spec\n\n## 1. Pricing\n\nSettled.\n"), (0, []))

    def test_none_with_no_inline_is_clear(self):
        text = "# Spec\n\n## Open items\n\nNone.\n\n## 1. Pricing\n\nSettled.\n"
        self.assertEqual(sm.check(text), (0, []))

    def test_index_only_names_each_entry(self):
        _, failures = sm.check(INDEX_ONLY)
        self.assertEqual(len(failures), 2)
        self.assertTrue(all("no inline occurrence" in f for f in failures))
        self.assertIn("line 5", failures[0])
        self.assertIn("§3 Rollout", failures[1])

    def test_inline_marker_with_no_entry(self):
        _, failures = sm.check(UNLISTED)
        self.assertEqual(len(failures), 1)
        self.assertIn("line 13: inline NEEDS-FEEDBACK has no index entry", failures[0])

    def test_markers_without_an_index(self):
        count, failures = sm.check(NO_INDEX)
        self.assertEqual(count, 1)
        self.assertEqual(len(failures), 1)
        self.assertIn("no open-items index", failures[0])

    def test_index_says_none_while_inline_remains(self):
        _, failures = sm.check(SAYS_NONE)
        self.assertTrue(any("says None" in f for f in failures))
        self.assertTrue(any("has no index entry" in f for f in failures))

    def test_preamble_list_is_the_index(self):
        self.assertEqual(sm.check(PREAMBLE_LIST), (1, []))

    def test_kind_must_agree(self):
        _, failures = sm.check(KIND_MISMATCH)
        self.assertEqual(len(failures), 2)
        self.assertIn("index entry NEEDS-FEEDBACK has no inline", failures[0])
        self.assertIn("inline NEEDS-CONFIRM has no index entry", failures[1])

    def test_fenced_marker_is_ignored(self):
        text = "# Spec\n\n```\nNEEDS-CONFIRM example\n```\n"
        self.assertEqual(sm.check(text), (0, []))

    def test_title_alone_matches(self):
        text = (
            "# Spec\n\n## Open items\n\n- NEEDS-CONFIRM the Pricing model\n\n"
            "## Pricing\n\nNEEDS-CONFIRM: fair = FX × basis.\n"
        )
        self.assertEqual(sm.check(text), (1, []))

    def test_explicit_reference_does_not_fall_back_to_title_words(self):
        # One inline marker in §2 must not satisfy an entry citing §3 merely
        # because the §3 entry's text mentions pricing.
        text = (
            "# Spec\n\n## Open items\n\n- NEEDS-CONFIRM §2 Pricing basis\n"
            "- NEEDS-CONFIRM §3 Rollout of the new pricing order\n\n"
            "## 2. Pricing\n\nNEEDS-CONFIRM: fair = FX × basis.\n\n"
            "## 3. Rollout\n\nAUD first.\n"
        )
        _, failures = sm.check(text)
        self.assertEqual(len(failures), 1)
        self.assertIn("§3 Rollout", failures[0])

    def test_entry_covers_a_subsection_marker(self):
        text = (
            "# Spec\n\n## Open items\n\n- NEEDS-CONFIRM §2 Pricing\n\n"
            "## 2. Pricing\n\n### 2.1 Fees\n\nNEEDS-CONFIRM: a 2 bps fee.\n"
        )
        self.assertEqual(sm.check(text), (1, []))

    def test_lazy_continuation_stays_in_preamble_list(self):
        text = (
            "# Spec\n\n- NEEDS-CONFIRM §1 Pricing which wraps\nlazily here\n"
            "- NEEDS-FEEDBACK §2 Rollout\n\n## 1. Pricing\n\nNEEDS-CONFIRM: x.\n\n"
            "## 2. Rollout\n\nNEEDS-FEEDBACK: y.\n"
        )
        self.assertEqual(sm.check(text), (2, []))

    def test_none_under_one_group_is_not_the_index_saying_none(self):
        text = (
            "# Spec\n\n## Open items\n\n### Confirm\n\n- NEEDS-CONFIRM §1 Pricing\n\n"
            "### Feedback\n\n- None.\n\n## 1. Pricing\n\nNEEDS-CONFIRM: x.\n"
        )
        self.assertEqual(sm.check(text), (1, []))

    def test_unclosed_fence_hiding_a_marker_fails(self):
        text = "# Spec\n\n```\n## Open items\n\n- NEEDS-CONFIRM §1 Pricing\n"
        _, failures = sm.check(text)
        self.assertEqual(len(failures), 1)
        self.assertIn("line 3: code fence never closed", failures[0])

    def test_longer_fence_closes_only_on_its_own_length(self):
        text = "# Spec\n\n````\n```\nNEEDS-CONFIRM example\n```\n````\n"
        self.assertEqual(sm.check(text), (0, []))

    def test_two_entries_cannot_share_one_inline_marker(self):
        # §2 covers a marker under §2.1, but that marker is §2.1's own entry's;
        # the §2 entry still needs one of its own.
        text = (
            "# Spec\n\n## Open items\n\n- NEEDS-CONFIRM §2 Pricing model\n"
            "- NEEDS-CONFIRM §2.1 Fee level\n\n## 2. Pricing\n\nFX × basis.\n\n"
            "### 2.1 Fees\n\nNEEDS-CONFIRM: a 2 bps fee.\n"
        )
        _, failures = sm.check(text)
        self.assertEqual(len(failures), 1)
        self.assertIn("§2 Pricing model", failures[0])

    def test_reference_to_no_section_falls_back_to_title(self):
        text = (
            "# Spec\n\n## Open items\n\n- NEEDS-CONFIRM §1 Pricing\n\n"
            "## Pricing\n\nNEEDS-CONFIRM: fair = FX × basis.\n"
        )
        self.assertEqual(sm.check(text), (1, []))

    def test_level_one_sections_without_a_title(self):
        text = (
            "- NEEDS-CONFIRM §1 Pricing\n\n# 1. Pricing\n\nNEEDS-CONFIRM: x.\n\n"
            "# 2. Rollout\n\nSettled.\n"
        )
        self.assertEqual(sm.check(text), (1, []))

    def test_level_one_open_items_ends_at_the_section_level(self):
        text = (
            "# Open items\n\n- NEEDS-CONFIRM §1 Pricing\n\n## 1. Pricing\n\n"
            "NEEDS-CONFIRM: x.\n"
        )
        self.assertEqual(sm.check(text), (1, []))

    def test_info_string_line_does_not_close_a_fence(self):
        text = "# Spec\n\n```\n```rust\nNEEDS-CONFIRM example\n```\n"
        self.assertEqual(sm.check(text), (0, []))

    def test_title_below_front_matter_is_not_a_section(self):
        text = (
            "Status: draft\n\n# Spec\n\n- NEEDS-CONFIRM §1 Pricing\n\n"
            "## 1. Pricing\n\nNEEDS-CONFIRM: x.\n"
        )
        self.assertEqual(sm.check(text), (1, []))


class RunTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def spec(self, text: str) -> str:
        path = os.path.join(self.tmp.name, "spec.md")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
        return path

    def invoke(self, *argv: str) -> tuple[int, str]:
        out = io.StringIO()
        with redirect_stdout(out), redirect_stderr(io.StringIO()):
            code = sm.run(["spec_markers.py", *argv])
        return code, out.getvalue()

    def test_clear_prints_one_line(self):
        code, out = self.invoke(self.spec(CLEAR))
        self.assertEqual(code, 0)
        self.assertEqual(out, "spec-markers | 3 open item(s); index and inline agree\n")

    def test_quiet_prints_nothing(self):
        self.assertEqual(self.invoke("--quiet", self.spec(CLEAR)), (0, ""))

    def test_failure_exits_one_naming_each(self):
        code, out = self.invoke("--quiet", self.spec(INDEX_ONLY))
        self.assertEqual(code, 1)
        self.assertEqual(len(out.splitlines()), 2)

    def test_unreadable_exits_two(self):
        code, _ = self.invoke(os.path.join(self.tmp.name, "missing.md"))
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()
