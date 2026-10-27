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

    def test_fenced_marker_is_ignored(self):
        text = "# Spec\n\n```\nNEEDS-CONFIRM example\n```\n"
        self.assertEqual(sm.check(text), (0, []))


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
