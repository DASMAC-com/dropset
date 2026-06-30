"""Stdlib ``unittest`` tests for the merge-tasks consolidation helper.

Run from ``.claude/tools`` with ``python3 -m unittest`` (or via the repo's
``make tools-tests``).
"""

import unittest

from merge_tasks import (
    MergeTasksError,
    assemble,
    extract_touches,
    is_meta_glob,
    parse_token,
    plan,
    strip_claude_prefix,
)


class ParseTests(unittest.TestCase):
    def test_parse_token_forms(self):
        self.assertEqual(parse_token("615"), 615)
        self.assertEqual(parse_token("ENG-615"), 615)
        self.assertEqual(parse_token("eng-615"), 615)
        self.assertEqual(parse_token("  ENG-7 "), 7)

    def test_parse_token_rejects_garbage(self):
        with self.assertRaises(MergeTasksError):
            parse_token("nope")

    def test_plan_dedups_and_defaults_lowest_survivor(self):
        out = plan(["622", "615", "ENG-615", "624"], None)
        self.assertEqual(out["survivor"], "ENG-615")
        self.assertEqual(out["ids"], ["ENG-615", "ENG-622", "ENG-624"])

    def test_plan_dedups_repeated(self):
        out = plan(["622", "823", "823"], None)
        self.assertEqual(out["ids"], ["ENG-622", "ENG-823"])

    def test_plan_survivor_override(self):
        out = plan(["615", "622"], 622)
        self.assertEqual(out["survivor"], "ENG-622")

    def test_plan_override_must_be_in_set(self):
        with self.assertRaises(MergeTasksError):
            plan(["615", "622"], 999)

    def test_plan_needs_two(self):
        with self.assertRaises(MergeTasksError):
            plan(["615"], None)
        with self.assertRaises(MergeTasksError):
            plan(["615", "ENG-615"], None)  # dedups to one


class TouchesTests(unittest.TestCase):
    def test_extract_touches_strips_line_and_collects_globs(self):
        body = "Intro.\n\n**Touches**: `tui/`, sdk/rs/**\n"
        clean, globs = extract_touches(body)
        self.assertNotIn("**Touches**:", clean)
        self.assertEqual(globs, ["tui/", "sdk/rs/**"])

    def test_extract_touches_keeps_fingerprint(self):
        body = "**Fingerprint**: a:b\n**Touches**: x/\n"
        clean, globs = extract_touches(body)
        self.assertIn("**Fingerprint**: a:b", clean)
        self.assertEqual(globs, ["x/"])

    def test_is_meta_glob(self):
        self.assertTrue(is_meta_glob(".claude/skills/x"))
        self.assertTrue(is_meta_glob("CLAUDE.md"))
        self.assertTrue(is_meta_glob("docs/conventions/**"))
        self.assertTrue(is_meta_glob("tools/stage-backlog/**"))
        self.assertFalse(is_meta_glob("programs/dropset/src/lib.rs"))
        self.assertFalse(is_meta_glob("docs/indexer.md"))

    def test_strip_claude_prefix(self):
        self.assertEqual(strip_claude_prefix("Claude: Do x"), "Do x")
        self.assertEqual(strip_claude_prefix("Do x"), "Do x")


class AssembleTests(unittest.TestCase):
    def _issues(self):
        return {
            "survivor": "ENG-615",
            "issues": [
                {
                    "id": "ENG-615",
                    "number": 615,
                    "title": "Refine the audit dedup",
                    "description": (
                        "Survivor intro.\n\n**Fingerprint**: audit:dedup\n"
                        "**Touches**: .claude/skills/audit/**\n"
                    ),
                },
                {
                    "id": "ENG-622",
                    "number": 622,
                    "title": "Claude: Tweak stage-backlog",
                    "description": (
                        "Folded body.\n\n**Fingerprint**: stage:tweak\n"
                        "**Touches**: tools/stage-backlog/**\n"
                    ),
                },
            ],
        }

    def test_folds_as_part_section_preserving_fingerprints(self):
        out = assemble(self._issues())
        self.assertIn("# Part 1 — Tweak stage-backlog", out["description"])
        # both fingerprints survive
        self.assertIn("**Fingerprint**: audit:dedup", out["description"])
        self.assertIn("**Fingerprint**: stage:tweak", out["description"])
        # the folded issue's Claude: title prefix is stripped in the heading
        self.assertNotIn("# Part 1 — Claude:", out["description"])

    def test_unions_touches_into_one_line(self):
        out = assemble(self._issues())
        self.assertEqual(
            out["touches"], [".claude/skills/audit/**", "tools/stage-backlog/**"]
        )
        # exactly one consolidated Touches line, at the end
        self.assertEqual(out["description"].count("**Touches**:"), 1)
        self.assertTrue(
            out["description"]
            .rstrip()
            .endswith("**Touches**: .claude/skills/audit/**, tools/stage-backlog/**")
        )

    def test_all_meta_applies_claude_prefix(self):
        out = assemble(self._issues())
        self.assertTrue(out["all_meta"])
        self.assertEqual(out["title"], "Claude: Refine the audit dedup")
        self.assertFalse(out["cross_area"])

    def test_cross_area_when_mixing_meta_and_product(self):
        data = self._issues()
        data["issues"][1]["description"] = (
            "Body.\n\n**Touches**: programs/dropset/src/lib.rs\n"
        )
        out = assemble(data)
        self.assertFalse(out["all_meta"])
        self.assertTrue(out["cross_area"])
        # a non-meta union means no auto-prefix
        self.assertEqual(out["title"], "Refine the audit dedup")

    def test_no_touch_issue_withholds_prefix(self):
        # A folded issue with no **Touches**: can't be proven meta-work, so the
        # whole merge isn't all-meta and the Claude: prefix is withheld.
        data = self._issues()
        data["issues"][1]["description"] = "Folded body, no touches.\n"
        out = assemble(data)
        self.assertFalse(out["all_meta"])
        self.assertEqual(out["title"], "Refine the audit dedup")
        # not cross-area either: a no-touch issue is neither meta nor product
        self.assertFalse(out["cross_area"])

    def test_survivor_must_be_present(self):
        data = self._issues()
        data["survivor"] = "ENG-999"
        with self.assertRaises(MergeTasksError):
            assemble(data)


if __name__ == "__main__":
    unittest.main()
