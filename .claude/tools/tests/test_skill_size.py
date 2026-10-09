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

    def test_every_wrapped_form_is_measured_in_full(self):
        # Under-measuring a wrapped description is the gate's one fail-open
        # path, so each way of wrapping a long line must fold its body.
        forms = {
            "plain continued": "description: first line\n  second\n",
            "empty then indented": "description:\n  first line\n  second\n",
            "indent then chomp": "description: >2-\n  first line\n  second\n",
            "header comment": "description: | # note\n  first line\n  second\n",
        }
        for label, field in forms.items():
            with self.subTest(form=label):
                text = f"---\nname: x\n{field}user-invocable: true\n---\n"
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

    def test_an_over_cap_project_file_fails_at_its_own_cap(self):
        self.put("CLAUDE.md", "p" * (ss.PROJECT_CAP + 1))
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"}}
        )
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 1)
        self.assertIn(f"CLAUDE.md: {ss.PROJECT_CAP + 1:,} bytes", err)

    def test_a_malformed_ceiling_fails_and_holds_the_subject_to_the_cap(self):
        for ceiling in (-1, "big", True, None):
            with self.subTest(ceiling=ceiling):
                self.write_baseline(
                    {self.big_key(): {"ceiling": ceiling, "issue": "ENG-1"}}
                )
                code, _, err = self.run_tool("--check")
                self.assertEqual(code, 1)
                self.assertIn("is not a byte count", err)
                self.assertIn(f"{self.big_key()}: {ss.ENTRY_CAP + 500:,} bytes", err)

    def test_a_non_object_baseline_fails_without_a_traceback(self):
        for body in ("{", "[]", '{"exceptions": ["ab"]}', '{"exceptions": {"k": 1}}'):
            with self.subTest(body=body):
                self.baseline.write_text(body, encoding="utf-8")
                code, _, err = self.run_tool("--check")
                self.assertEqual(code, 1)
                self.assertNotIn("Traceback", err)

    def test_slack_beyond_the_headroom_is_a_notice_not_a_failure(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": 2 * ss.ENTRY_CAP, "issue": "ENG-1"}}
        )
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 0)
        self.assertIn(f"note: {self.big_key()}", err)

    def test_slack_within_the_headroom_is_quiet(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 900, "issue": "ENG-1"}}
        )
        code, _, err = self.run_tool("--check")
        self.assertEqual(code, 0)
        self.assertEqual(err, "")


class Headroom(unittest.TestCase):
    def test_headroom_is_ten_percent_rounded_up(self):
        self.assertEqual(ss.headroom_ceiling(1_000), 1_100)
        self.assertEqual(ss.headroom_ceiling(1_001), 1_102)
        self.assertEqual(ss.headroom_ceiling(0), 0)

    def test_a_fresh_ratchet_reads_below_the_watch_threshold(self):
        for size in (1, 999, 32_001, 250_944):
            ceiling = ss.headroom_ceiling(size)
            self.assertLessEqual(100 * size / ceiling, ss.WATCH_PERCENT)


class Write(Fixture):
    def test_write_lowers_a_ceiling_to_the_size_plus_headroom(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": 2 * ss.ENTRY_CAP, "issue": "ENG-1"}}
        )
        self.assertEqual(self.run_tool("--write")[0], 0)
        stored = json.loads(self.baseline.read_text())["exceptions"]
        size = ss.ENTRY_CAP + 500
        self.assertEqual(
            stored[self.big_key()],
            {"ceiling": -(-size * 11 // 10), "issue": "ENG-1"},
        )

    def test_a_ratcheted_file_may_grow_within_its_headroom_only(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": 2 * ss.ENTRY_CAP, "issue": "ENG-1"}}
        )
        self.run_tool("--write")
        ceiling = ss.headroom_ceiling(ss.ENTRY_CAP + 500)
        self.put(self.big_key(), skill("short", ceiling))
        self.assertEqual(self.run_tool("--check")[0], 0)
        self.put(self.big_key(), skill("short", ceiling + 1))
        self.assertEqual(self.run_tool("--check")[0], 1)

    def test_a_shrink_within_the_headroom_keeps_the_old_ceiling(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": ss.ENTRY_CAP + 900, "issue": "ENG-1"}}
        )
        self.run_tool("--write")
        stored = json.loads(self.baseline.read_text())["exceptions"]
        self.assertEqual(stored[self.big_key()]["ceiling"], ss.ENTRY_CAP + 900)

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

    def test_write_drops_an_entry_whose_subject_is_gone(self):
        self.write_baseline(
            {
                self.big_key(): {"ceiling": ss.ENTRY_CAP + 500, "issue": "ENG-1"},
                "gone/SKILL.md": {"ceiling": ss.ENTRY_CAP + 9, "issue": "ENG-1"},
            }
        )
        self.run_tool("--write")
        stored = json.loads(self.baseline.read_text())["exceptions"]
        self.assertEqual(list(stored), [self.big_key()])

    def test_write_refuses_a_malformed_baseline_and_leaves_it_alone(self):
        body = json.dumps({"exceptions": {self.big_key(): {"ceiling": "x"}}})
        self.baseline.write_text(body, encoding="utf-8")
        code, _, err = self.run_tool("--write")
        self.assertEqual(code, 2)
        self.assertNotIn("Traceback", err)
        self.assertEqual(self.baseline.read_text(), body)


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

    def test_admit_refuses_a_duplicate_request(self):
        code, _, err = self.run_tool(
            "--write",
            "--admit",
            f"{self.big_key()}=ENG-7",
            "--admit",
            f"{self.big_key()}=ENG-8",
        )
        self.assertEqual(code, 2)
        self.assertIn("named twice", err)
        self.assertFalse(self.baseline.exists())

    def test_admit_refuses_an_unknown_subject(self):
        code, _, err = self.run_tool("--write", "--admit", "nope/SKILL.md=ENG-7")
        self.assertEqual(code, 2)
        self.assertIn("no such subject", err)


class Report(Fixture):
    def test_report_ranks_entries_and_counts_siblings(self):
        code, out, _ = self.run_tool("--report")
        self.assertEqual(code, 0)
        lines = out.splitlines()
        self.assertTrue(lines[0].startswith("* big"))
        self.assertIn(f"siblings {ss.ENTRY_CAP + 1:>8,} (1)", lines[1])

    def test_report_ignores_a_malformed_baseline(self):
        self.baseline.write_text("[]", encoding="utf-8")
        self.assertEqual(self.run_tool("--report")[0], 0)


class Utilization(Fixture):
    def test_every_subject_is_listed_highest_percent_first(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": 2 * ss.ENTRY_CAP, "issue": "ENG-1"}}
        )
        code, out, err = self.run_tool("--utilization")
        self.assertEqual((code, err), (0, ""))
        lines = out.splitlines()
        keys = [line.split()[-1] for line in lines]
        self.assertEqual(
            sorted(keys),
            sorted(subject.key for subject in ss.collect(self.root)),
        )
        percents = [float(line[1:].split("%")[0]) for line in lines]
        self.assertEqual(percents, sorted(percents, reverse=True))

    def test_the_limit_is_the_greater_of_cap_and_ceiling(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": 2 * ss.ENTRY_CAP, "issue": "ENG-1"}}
        )
        _, out, _ = self.run_tool("--utilization")
        big = next(line for line in out.splitlines() if line.endswith(self.big_key()))
        self.assertEqual(
            big.split()[1:4], [f"{ss.ENTRY_CAP + 500:,}", "/", f"{2 * ss.ENTRY_CAP:,}"]
        )

    def test_a_subject_above_the_watch_threshold_fails_and_is_named(self):
        # With no baseline entry, the big file is over its cap: above the watch.
        code, out, err = self.run_tool("--utilization")
        self.assertEqual(code, 1)
        self.assertIn(f"skill-size:   {self.big_key()}", err)
        self.assertNotIn(".claude/skills/small/SKILL.md\n", err)
        self.assertTrue(out.splitlines()[0].startswith("!"))

    def test_the_threshold_is_strictly_above(self):
        at = ss.ENTRY_CAP * ss.WATCH_PERCENT // 100
        self.put(self.big_key(), skill("short", at))
        self.assertEqual(self.run_tool("--utilization")[0], 0)
        self.put(self.big_key(), skill("short", at + 1))
        self.assertEqual(self.run_tool("--utilization")[0], 1)

    def test_a_fresh_ratchet_reads_clean(self):
        self.write_baseline(
            {self.big_key(): {"ceiling": 2 * ss.ENTRY_CAP, "issue": "ENG-1"}}
        )
        self.run_tool("--write")
        self.assertEqual(self.run_tool("--utilization")[0], 0)

    def test_a_malformed_baseline_is_refused(self):
        self.baseline.write_text("[]", encoding="utf-8")
        code, out, err = self.run_tool("--utilization")
        self.assertEqual((code, out), (2, ""))
        self.assertNotIn("Traceback", err)


if __name__ == "__main__":
    unittest.main()
