"""Stdlib ``unittest`` tests for the merge-tasks consolidation helper.

Run via the repo's ``make tools-tests`` (discovery adds ``.claude/tools`` as
the top-level dir so the bare ``import merge_tasks`` below resolves).
"""

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stdout

from merge_tasks import (
    MergeTasksError,
    assemble,
    build_patch_ops,
    extract_touches,
    is_meta_glob,
    parse_token,
    plan,
    raw_touches_lines,
    run,
    strip_claude_prefix,
)


def apply_ops(body: str, ops: list[dict]) -> str:
    """Apply Linear ``patch`` ops the way the API does — in order, each anchor
    matching exactly once — so a test can assert the ops and the wholesale
    description converge on the same text.

    Only the two ops this tool emits (``replace``, ``append``) are handled; an
    unexpected op is an error rather than a silent no-op.
    """
    out = body
    for op in ops:
        kind = op["op"]
        if kind == "append":
            out += op["text"]
        elif kind == "replace":
            old = op["old_string"]
            if out.count(old) != 1:
                raise AssertionError(
                    f"anchor matched {out.count(old)} times, not once: {old!r}"
                )
            out = out.replace(old, op["new_string"])
        else:
            raise AssertionError(f"unexpected op kind: {kind}")
    return out


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
        self.assertTrue(is_meta_glob(".claude/tools/**"))
        self.assertFalse(is_meta_glob("programs/dropset/src/lib.rs"))
        self.assertFalse(is_meta_glob("docs/indexer.md"))
        # the relocated build script's home is product-adjacent, not meta-work
        self.assertFalse(is_meta_glob("brand-assets/**"))

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
                    "title": "Claude: Tweak sync-blockers",
                    "description": (
                        "Folded body.\n\n**Fingerprint**: stage:tweak\n"
                        "**Touches**: .claude/tools/**\n"
                    ),
                },
            ],
        }

    def test_folds_as_part_section_preserving_fingerprints(self):
        out = assemble(self._issues())
        self.assertIn("# Part 1 — Tweak sync-blockers", out["description"])
        # both fingerprints survive
        self.assertIn("**Fingerprint**: audit:dedup", out["description"])
        self.assertIn("**Fingerprint**: stage:tweak", out["description"])
        # the folded issue's Claude: title prefix is stripped in the heading
        self.assertNotIn("# Part 1 — Claude:", out["description"])

    def test_unions_touches_into_one_line(self):
        out = assemble(self._issues())
        self.assertEqual(
            out["touches"], [".claude/skills/audit/**", ".claude/tools/**"]
        )
        # exactly one consolidated Touches line, at the end
        self.assertEqual(out["description"].count("**Touches**:"), 1)
        self.assertTrue(
            out["description"]
            .rstrip()
            .endswith("**Touches**: .claude/skills/audit/**, .claude/tools/**")
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

    def test_emits_patch_ops_alongside_the_wholesale_body(self):
        out = assemble(self._issues())
        self.assertEqual(out["patch_fallback_reason"], "")
        kinds = [op["op"] for op in out["patch_ops"]]
        # delete the survivor's Touches line, append the part, re-append the union
        self.assertEqual(kinds, ["replace", "append", "append"])

    def test_no_op_carries_the_survivors_existing_body(self):
        """The whole point: the survivor's text is never re-sent. Only its short
        Touches line may appear, as the replace anchor."""
        out = assemble(self._issues())
        for op in out["patch_ops"]:
            payload = op.get("text", "") + op.get("old_string", "")
            self.assertNotIn("Survivor intro.", payload)
            self.assertNotIn("**Fingerprint**: audit:dedup", payload)

    def test_ops_reproduce_the_wholesale_description(self):
        data = self._issues()
        out = assemble(data)
        stored = data["issues"][0]["description"]
        self.assertEqual(
            apply_ops(stored, out["patch_ops"]).strip(),
            out["description"].strip(),
        )

    def test_ops_still_work_when_the_survivor_has_no_touches_line(self):
        """No anchor needed at all — a pure-append fold."""
        data = self._issues()
        data["issues"][0]["description"] = "Survivor with no touches.\n"
        out = assemble(data)
        self.assertEqual(out["patch_fallback_reason"], "")
        self.assertEqual([op["op"] for op in out["patch_ops"]], ["append", "append"])
        self.assertEqual(
            apply_ops(data["issues"][0]["description"], out["patch_ops"]).strip(),
            out["description"].strip(),
        )

    def test_falls_back_when_the_survivor_has_two_touches_lines(self):
        data = self._issues()
        data["issues"][0]["description"] = (
            "Survivor.\n\n**Touches**: a/\n\n**Touches**: b/\n"
        )
        out = assemble(data)
        self.assertIsNone(out["patch_ops"])
        self.assertIn("ambiguous", out["patch_fallback_reason"])
        # the wholesale body is still there to fall back on
        self.assertIn("# Part 1 —", out["description"])

    def test_falls_back_when_the_touches_anchor_carries_an_eng_tag(self):
        """Linear stores ENG-### as a mention node, so such an anchor can never
        match the stored text."""
        data = self._issues()
        data["issues"][0]["description"] = (
            "Survivor.\n\n**Touches**: docs/ENG-615-notes.md\n"
        )
        out = assemble(data)
        self.assertIsNone(out["patch_ops"])
        self.assertIn("mention node", out["patch_fallback_reason"])

    def test_a_part_heading_with_a_tag_is_appended_not_anchored(self):
        """A folded title may carry an ENG tag; that only matters for anchors,
        and no anchor is ever built from a title."""
        data = self._issues()
        data["issues"][1]["title"] = "Follow up on ENG-700 feeds"
        out = assemble(data)
        self.assertEqual(out["patch_fallback_reason"], "")
        appends = [op for op in out["patch_ops"] if op["op"] == "append"]
        self.assertTrue(any("ENG-700" in op["text"] for op in appends))
        replaces = [op for op in out["patch_ops"] if op["op"] == "replace"]
        for op in replaces:
            self.assertNotIn("ENG-700", op["old_string"])

    def test_anchor_uses_the_stored_line_verbatim_including_backticks(self):
        data = self._issues()
        data["issues"][0]["description"] = "Survivor.\n\n**Touches**: `tui/`\n"
        out = assemble(data)
        replaces = [op for op in out["patch_ops"] if op["op"] == "replace"]
        self.assertEqual(replaces[0]["old_string"], "\n**Touches**: `tui/`")


class RawTouchesLineTests(unittest.TestCase):
    """The anchor is built from the stored line, not the parsed globs."""

    def test_returns_the_line_verbatim(self):
        body = "x\n**Touches**: `tui/`, sdk/rs/**\ny\n"
        self.assertEqual(raw_touches_lines(body), ["**Touches**: `tui/`, sdk/rs/**"])

    def test_collects_a_list_marker_line(self):
        self.assertEqual(
            raw_touches_lines("- **Touches**: a/\n"), ["- **Touches**: a/"]
        )

    def test_none_when_absent(self):
        self.assertEqual(raw_touches_lines("no fields here\n"), [])


class PatchOpCapTests(unittest.TestCase):
    """Linear caps `patch` at 50 ops; a wider fold goes wholesale rather than
    being rejected mid-merge."""

    def test_falls_back_over_the_fifty_op_cap(self):
        sections = [f"---\n\n# Part {n} — t\n\nbody" for n in range(60)]
        ops, reason = build_patch_ops("Survivor.\n", sections, ["a/"])
        self.assertIsNone(ops)
        self.assertIn("50-op cap", reason)

    def test_just_under_the_cap_is_fine(self):
        sections = [f"---\n\n# Part {n} — t\n\nbody" for n in range(48)]
        ops, reason = build_patch_ops("Survivor.\n", sections, ["a/"])
        self.assertEqual(reason, "")
        self.assertEqual(len(ops), 49)

    def test_an_empty_op_list_falls_back_with_a_reason(self):
        """`patch` requires at least one op, and a caller testing `if patch_ops:`
        would otherwise fall through to wholesale with an empty reason."""
        ops, reason = build_patch_ops("Survivor with no touches.\n", [], [])
        self.assertIsNone(ops)
        self.assertIn("no operations", reason)


class PatchOpNewlineTests(unittest.TestCase):
    """The lead-newline arithmetic, across every trailing-newline count."""

    def _merged(self, stored, sections, globs):
        ops, reason = build_patch_ops(stored, sections, globs)
        self.assertEqual(reason, "")
        return apply_ops(stored, ops)

    def test_no_trailing_newline(self):
        got = self._merged("Survivor.", ["---\n\n# Part 1 — t\n\nbody"], ["a/"])
        self.assertIn("Survivor.\n\n---", got)

    def test_one_trailing_newline(self):
        got = self._merged("Survivor.\n", ["---\n\n# Part 1 — t\n\nbody"], ["a/"])
        self.assertIn("Survivor.\n\n---", got)

    def test_two_trailing_newlines(self):
        got = self._merged("Survivor.\n\n", ["---\n\n# Part 1 — t\n\nbody"], ["a/"])
        self.assertIn("Survivor.\n\n---", got)

    def test_three_or_more_trailing_newlines_is_a_known_divergence(self):
        """The `max(0, 2 - trailing)` clamp cannot remove newlines already there,
        so ops keep them where the wholesale path would rstrip. Documented
        compromise — pinned here so it can't drift into a surprise."""
        got = self._merged("Survivor.\n\n\n\n", ["---\n\n# Part 1 — t\n\nbody"], ["a/"])
        self.assertIn("Survivor.\n\n\n\n---", got)

    def test_a_first_line_touches_can_still_anchor(self):
        """Prefixing the anchor with a newline would leave a Touches-first body
        with no anchor at all, and misreport it as ambiguous."""
        stored = "**Touches**: a/\n\nSurvivor prose.\n"
        ops, reason = build_patch_ops(stored, ["---\n\n# Part 1 — t\n\nbody"], ["a/"])
        self.assertEqual(reason, "")
        merged = apply_ops(stored, ops)
        self.assertEqual(merged.count("**Touches**:"), 1)
        self.assertIn("Survivor prose.", merged)


class AssembleCliTests(unittest.TestCase):
    """The ``--out`` file-handoff lives in ``run()``, not ``assemble()``."""

    def _issues(self):
        return {
            "survivor": "ENG-615",
            "issues": [
                {
                    "id": "ENG-615",
                    "number": 615,
                    "title": "Refine the audit dedup",
                    "description": "Survivor.\n\n**Touches**: .claude/skills/audit/**\n",
                },
                {
                    "id": "ENG-622",
                    "number": 622,
                    "title": "Tweak sync-blockers",
                    "description": "Folded.\n\n**Touches**: .claude/tools/**\n",
                },
            ],
        }

    def _run_capture(self, argv):
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = run(argv)
        return rc, json.loads(buf.getvalue())

    def test_without_out_prints_description_inline(self):
        with tempfile.TemporaryDirectory() as d:
            issues = os.path.join(d, "issues.json")
            with open(issues, "w", encoding="utf-8") as fh:
                json.dump(self._issues(), fh)
            rc, out = self._run_capture(["merge_tasks.py", "assemble", issues])
        self.assertEqual(rc, 0)
        self.assertIn("description", out)
        self.assertNotIn("description_path", out)

    def test_out_writes_body_and_omits_it_from_stdout(self):
        with tempfile.TemporaryDirectory() as d:
            issues = os.path.join(d, "issues.json")
            body = os.path.join(d, "body.md")
            with open(issues, "w", encoding="utf-8") as fh:
                json.dump(self._issues(), fh)
            rc, out = self._run_capture(
                ["merge_tasks.py", "assemble", issues, "--out", body]
            )
            self.assertEqual(rc, 0)
            # the large body is gone from stdout, replaced by its path
            self.assertNotIn("description", out)
            self.assertEqual(out["description_path"], body)
            # metadata still rides inline
            self.assertEqual(out["title"], "Claude: Refine the audit dedup")
            self.assertTrue(out["all_meta"])
            self.assertEqual(
                out["touches"], [".claude/skills/audit/**", ".claude/tools/**"]
            )
            # and the file holds the merged body
            with open(body, encoding="utf-8") as fh:
                written = fh.read()
            self.assertIn("# Part 1 — Tweak sync-blockers", written)
            self.assertTrue(written.rstrip().endswith(".claude/tools/**"))

    def test_ops_out_writes_ops_and_omits_them_from_stdout(self):
        with tempfile.TemporaryDirectory() as d:
            issues = os.path.join(d, "issues.json")
            body = os.path.join(d, "body.md")
            ops_path = os.path.join(d, "ops.json")
            with open(issues, "w", encoding="utf-8") as fh:
                json.dump(self._issues(), fh)
            rc, out = self._run_capture(
                [
                    "merge_tasks.py",
                    "assemble",
                    issues,
                    "--out",
                    body,
                    "--ops-out",
                    ops_path,
                ]
            )
            self.assertEqual(rc, 0)
            self.assertNotIn("patch_ops", out)
            self.assertEqual(out["patch_ops_path"], ops_path)
            self.assertEqual(out["patch_ops_count"], 3)
            with open(ops_path, encoding="utf-8") as fh:
                ops = json.load(fh)
            self.assertEqual([op["op"] for op in ops], ["replace", "append", "append"])

    def test_ops_out_reports_no_path_when_the_fold_must_go_wholesale(self):
        data = self._issues()
        data["issues"][0]["description"] = (
            "Survivor.\n\n**Touches**: a/\n\n**Touches**: b/\n"
        )
        with tempfile.TemporaryDirectory() as d:
            issues = os.path.join(d, "issues.json")
            ops_path = os.path.join(d, "ops.json")
            with open(issues, "w", encoding="utf-8") as fh:
                json.dump(data, fh)
            rc, out = self._run_capture(
                ["merge_tasks.py", "assemble", issues, "--ops-out", ops_path]
            )
            self.assertEqual(rc, 0)
            self.assertIsNone(out["patch_ops_path"])
            self.assertEqual(out["patch_ops_count"], 0)
            self.assertIn("ambiguous", out["patch_fallback_reason"])
            # nothing was written, so the skill can't mistake a stale file for ops
            self.assertFalse(os.path.exists(ops_path))


if __name__ == "__main__":
    unittest.main()
