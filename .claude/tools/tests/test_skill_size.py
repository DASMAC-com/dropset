#!/usr/bin/env python3
"""Unit tests for ``skill_size.py`` (stdlib ``unittest``; no pytest).

The load-bearing property is the **freeze**: an over-cap subject in the
baseline may not grow by one byte, an under-cap subject may grow to the cap,
and ``--write`` only ever tightens. Each case builds a small embedded fixture
tree in a temp directory so nothing depends on the real skills' sizes.
"""

from __future__ import annotations

import io
import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import skill_size as ss


def skill(description: str, body_bytes: int) -> str:
    """A ``SKILL.md`` with a one-line description and a padded body."""
    head = f"---\nname: x\ndescription: {description}\n---\n\n"
    return head + "a" * max(0, body_bytes - len(head))


class Fixture(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "cfg").mkdir()
        self.baseline = self.root / "cfg" / "baseline.json"
        self.put("CLAUDE.md", "p" * 100)
        self.put(".claude/skills/big/SKILL.md", skill("short", ss.ENTRY_CAP + 500))
        self.put(".claude/skills/small/SKILL.md", skill("short", 1_000))
        # A sibling must never be measured as an entry file.
        self.put(".claude/skills/small/history.md", "h" * (ss.ENTRY_CAP + 1))

    def tearDown(self):
        self._tmp.cleanup()

    def put(self, rel: str, text: str) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def run_tool(self, *args: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = ss.main(
                ["--root", str(self.root), "--baseline", str(self.baseline), *args]
            )
        return code, out.getvalue(), err.getvalue()

    def write_baseline(self, exceptions: dict) -> None:
        self.baseline.write_text(
            json.dumps({"exceptions": exceptions}), encoding="utf-8"
        )

    def big_key(self) -> str:
        return ".claude/skills/big/SKILL.md"


class Collect(Fixture):
    def test_only_entry_files_and_descriptions_are_measured(self):
        keys = {s.key for s in ss.collect(self.root)}
        self.assertEqual(
            keys,
            {
                "CLAUDE.md",
                ".claude/skills/big/SKILL.md",
                ".claude/skills/big/SKILL.md#description",
                ".claude/skills/small/SKILL.md",
                ".claude/skills/small/SKILL.md#description",
            },
        )

    def test_entry_size_is_wc_c_bytes(self):
        sizes = {s.key: s.size for s in ss.collect(self.root)}
        self.assertEqual(sizes[self.big_key()], ss.ENTRY_CAP + 500)

    def test_description_bytes_count_utf8(self):
        self.put(".claude/skills/small/SKILL.md", skill("é" * 10, 200))
        sizes = {s.key: s.size for s in ss.collect(self.root)}
        self.assertEqual(sizes[".claude/skills/small/SKILL.md#description"], 20)


class DescriptionValue(unittest.TestCase):
    def test_plain_one_line(self):
        self.assertEqual(
            ss.description_value("---\ndescription: a b c\n---\n"), "a b c"
        )

    def test_folded_block_scalar(self):
        text = "---\ndescription: >-\n  first line\n  second\nname: x\n---\n"
        self.assertEqual(ss.description_value(text), "first line second")

    def test_no_frontmatter(self):
        self.assertIsNone(ss.description_value("# heading\ndescription: no\n"))

    def test_key_outside_frontmatter_is_ignored(self):
        self.assertIsNone(ss.description_value("---\nname: x\n---\ndescription: y\n"))


class Check(Fixture):
    def test_an_over_cap_file_with_no_exception_fails(self):
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 1)
        self.assertIn(f"{self.big_key()}: {ss.ENTRY_CAP + 500:,} bytes", err)

    def test_a_frozen_file_at_its_ceiling_passes(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"}}
        )
        self.assertEqual(self.run_tool("--check")[0], 0)

    def test_a_frozen_file_cannot_grow_by_one_byte(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 499, "issue": "ENG-1"}}
        )
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 1)
        self.assertIn("frozen until ENG-1", err)

    def test_a_stale_low_ceiling_still_allows_growth_to_the_cap(self):
        key = ".claude/skills/small/SKILL.md"
        self.write_baseline(
            {
                self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"},
                key: {"ceiling": 500, "issue": "ENG-1"},
            }
        )
        # 1,000 bytes is past the stale 500 ceiling but within the cap.
        self.assertEqual(self.run_tool("--check")[0], 0)

    def test_an_over_cap_ceiling_without_an_issue_is_refused(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": ""}}
        )
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 1)
        self.assertIn("names no retiring issue", err)

    def test_an_entry_for_a_missing_subject_fails(self):
        self.write_baseline(
            {
                self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"},
                "gone/SKILL.md": {"ceiling": 1, "issue": "ENG-1"},
            }
        )
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 1)
        self.assertIn("gone/SKILL.md: baseline entry names nothing", err)

    def test_an_over_cap_description_fails(self):
        self.put(
            ".claude/skills/small/SKILL.md",
            skill("d" * (ss.DESCRIPTION_CAP + 1), 2_000),
        )
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"}}
        )
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 1)
        self.assertIn("small/SKILL.md#description", err)


class Write(Fixture):
    def test_write_lowers_a_ceiling_to_the_current_size(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 900, "issue": "ENG-1"}}
        )
        self.assertEqual(self.run_tool("--write")[0], 0)
        stored = json.loads(self.baseline.read_text())["exceptions"]
        self.assertEqual(
            stored[self.big_key()], {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"}
        )

    def test_write_never_raises_a_ceiling(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 100, "issue": "ENG-1"}}
        )
        self.run_tool("--write")
        stored = json.loads(self.baseline.read_text())["exceptions"]
        self.assertEqual(stored[self.big_key()]["ceiling"], ss.ENTRY_CAP + 100)

    def test_write_drops_an_entry_once_under_the_cap(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"}}
        )
        self.put(self.big_key(), skill("short", ss.ENTRY_CAP - 1))
        self.run_tool("--write")
        self.assertEqual(json.loads(self.baseline.read_text())["exceptions"], {})

    def test_write_never_adds_an_entry_on_its_own(self):
        self.run_tool("--write")
        self.assertEqual(json.loads(self.baseline.read_text())["exceptions"], {})


class Admit(Fixture):
    def test_admit_freezes_at_the_current_size(self):
        code, _, _ = self.run_tool("--write", "--admit", f"{self.big_key()}=ENG-7")
        self.assertEqual(code, 0)
        stored = json.loads(self.baseline.read_text())["exceptions"]
        self.assertEqual(
            stored, {self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-7"}}
        )
        self.assertEqual(self.run_tool("--check")[0], 0)

    def test_admit_refuses_an_under_cap_subject(self):
        code, _, err = self.run_tool(
            "--write", "--admit", ".claude/skills/small/SKILL.md=ENG-7"
        )
        self.assertEqual(code, 2)
        self.assertIn("within its cap", err)
        self.assertFalse(self.baseline.exists())

    def test_admit_refuses_to_re_admit_a_frozen_subject(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 100, "issue": "ENG-1"}}
        )
        code, _, err = self.run_tool("--write", "--admit", f"{self.big_key()}=ENG-7")
        self.assertEqual(code, 2)
        self.assertIn("never raised", err)

    def test_admit_requires_an_issue_reference(self):
        code, _, err = self.run_tool("--write", "--admit", f"{self.big_key()}=soon")
        self.assertEqual(code, 2)
        self.assertIn("expected KEY=ENG-###", err)


class Report(Fixture):
    def test_report_ranks_entries_and_counts_siblings(self):
        code, out, _ = self.run_tool("--report")
        self.assertEqual(code, 0)
        lines = out.splitlines()
        self.assertTrue(lines[0].startswith("* big"))
        self.assertIn(f"siblings {ss.ENTRY_CAP + 1:>8,} (1)", lines[1])


if __name__ == "__main__":
    unittest.main()
