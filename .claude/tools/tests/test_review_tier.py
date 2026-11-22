"""Tests for review_tier.py: tier, lenses, gates and the integrity tagger.

Fixture file lists and hand-written diffs only — no git, no filesystem.
"""

from __future__ import annotations

import unittest

import review_tier as rt


def kinds_of(paths):
    """The docs / tests / source kind, by the same rules review_diff uses."""
    import review_diff

    return {p: review_diff.slice_for(p) for p in paths}


def files(*pairs):
    return [{"path": p, "changes": n} for p, n in pairs]


def diff(path, added=(), removed=(), start=1, new_file=False):
    """A one-hunk diff of ``path``."""
    head = f"diff --git a/{path} b/{path}\n"
    if new_file:
        head += "new file mode 100644\n--- /dev/null\n"
    else:
        head += f"--- a/{path}\n"
    head += f"+++ b/{path}\n"
    body = [f"-{t}" for t in removed] + [f"+{t}" for t in added]
    old = 0 if new_file else max(len(removed), 1)
    head += f"@@ -{start},{old} +{start},{len(added)} @@\n"
    return head + "\n".join(body) + "\n"


def tier_of(file_list, text, code_crates=1, test_ranges=None):
    paths = [f["path"] for f in file_list]
    return rt.classify_tier(
        file_list,
        kinds_of(paths),
        rt.parse_diff(text),
        test_ranges or {},
        code_crates,
    )["name"]


class ParseDiffTests(unittest.TestCase):
    def test_added_lines_carry_their_post_image_numbers(self):
        text = (
            "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n"
            "@@ -4,3 +4,4 @@\n ctx\n-old\n+new\n+more\n ctx\n"
        )
        (fd,) = rt.parse_diff(text)
        self.assertEqual(fd.path, "x.rs")
        self.assertEqual(fd.hunks[0].added, [(5, "new"), (6, "more")])
        self.assertEqual(fd.hunks[0].removed, ["old"])

    def test_a_header_before_the_first_file_is_ignored(self):
        text = "# review-diff fingerprint=abc head=def\n" + diff("a.py", ["x = 1"])
        (fd,) = rt.parse_diff(text)
        self.assertEqual(fd.path, "a.py")


class TestShapeTests(unittest.TestCase):
    def test_a_new_files_trailing_test_module_is_a_test_hunk(self):
        # The folded lever: one hunk from line 1 whose tail is #[cfg(test)].
        text = diff("tui/src/keys.rs", ["fn a() {}"] * 12, new_file=True)
        shape = rt.test_shape(
            rt.parse_diff(text), {"tui/src/keys.rs": "source"}, {"tui/src/keys.rs": [(8, 12)]}
        )
        self.assertEqual(shape["hunks"], 1)

    def test_a_source_hunk_outside_the_test_range_is_not(self):
        text = diff("src/lib.rs", ["fn a() {}"], start=2)
        shape = rt.test_shape(rt.parse_diff(text), {"src/lib.rs": "source"}, {"src/lib.rs": [(8, 12)]})
        self.assertEqual(shape, {"hunks": 0, "lines": 0})

    def test_a_test_file_counts_whole(self):
        text = diff("sdk/rs/tests/p.rs", ["fn p() {}"], ["fn q() {}"])
        shape = rt.test_shape(rt.parse_diff(text), {"sdk/rs/tests/p.rs": "tests"}, {})
        self.assertEqual(shape, {"hunks": 1, "lines": 2})


class TierTests(unittest.TestCase):
    def test_a_small_docs_edit_is_trivial(self):
        fl = files(("docs/a.md", 4))
        self.assertEqual(tier_of(fl, diff("docs/a.md", ["new"], ["old"])), "trivial")

    def test_a_small_edit_adding_control_flow_is_not_trivial(self):
        fl = files(("tui/src/ui.rs", 4))
        text = diff("tui/src/ui.rs", ["if x { draw(); }"], ["draw();"])
        self.assertNotEqual(tier_of(fl, text), "trivial")

    def test_a_program_path_is_never_trivial(self):
        fl = files(("programs/dropset/src/a.rs", 2))
        text = diff("programs/dropset/src/a.rs", ["const X: u8 = 2;"], ["const X: u8 = 1;"])
        self.assertNotEqual(tier_of(fl, text), "trivial")

    def test_a_ci_and_docker_diff_is_reduced_infra(self):
        fl = files((".github/workflows/x.yml", 80), ("feeds/Dockerfile", 40))
        text = diff(".github/workflows/x.yml", ["run: make"] * 80) + diff(
            "feeds/Dockerfile", ["RUN x"] * 40
        )
        self.assertEqual(tier_of(fl, text, code_crates=2), "reduced-infra")

    def test_a_meta_only_ci_like_diff_never_takes_infra(self):
        fl = files((".claude/skills/a/SKILL.md", 70), ("cfg/x.yml", 70))
        text = diff("cfg/x.yml", ["a: 1"] * 70)
        self.assertNotEqual(tier_of(fl, text, code_crates=2), "reduced-infra")

    def test_moved_code_is_reduced_move(self):
        body = [f"let v{i} = {i};" for i in range(40)]
        fl = files(("sdk/rs/src/a.rs", 40), ("sdk/rs/src/b.rs", 40))
        text = diff("sdk/rs/src/a.rs", removed=body) + diff(
            "sdk/rs/src/b.rs", added=body, new_file=True
        )
        self.assertEqual(tier_of(fl, text), "reduced-move")

    def test_reindented_code_is_reduced_reformat(self):
        old = [f"let v{i} = {i};" for i in range(40)]
        new = [f"    let v{i} = {i};" for i in range(40)]
        fl = files(("tui/src/a.rs", 80))
        self.assertEqual(tier_of(fl, diff("tui/src/a.rs", new, old)), "reduced-reformat")

    def test_value_swaps_are_reduced_rewiring(self):
        old = [f"const C{i}: u64 = {i};" for i in range(40)]
        new = [f"const C{i}: u64 = {i * 2 + 100};" for i in range(40)]
        fl = files(("tui/src/a.rs", 80))
        self.assertEqual(tier_of(fl, diff("tui/src/a.rs", new, old)), "reduced-rewiring")

    def test_inline_test_hunks_only_is_reduced_test_only(self):
        added = [f"    if x {{ assert!(y{i}); }}" for i in range(40)]
        fl = files(("tui/src/a.rs", 40))
        text = diff("tui/src/a.rs", added, start=50)
        self.assertEqual(
            tier_of(fl, text, test_ranges={"tui/src/a.rs": [(40, 200)]}),
            "reduced-test-only",
        )

    def test_one_crate_with_its_tests_is_small_single_crate(self):
        src = [f"if a{i} {{ b(); }}" for i in range(40)]
        tests = [f"fn t{i}() {{ if x {{}} }}" for i in range(40)]
        fl = files(("tui/src/a.rs", 40), ("tui/tests/a.rs", 40))
        text = diff("tui/src/a.rs", src) + diff("tui/tests/a.rs", tests)
        self.assertEqual(tier_of(fl, text), "small-single-crate")

    def test_a_large_tools_diff_is_meta(self):
        src = [f"    if a{i}: return {i}" for i in range(400)]
        fl = files((".claude/tools/a.py", 400), ("CLAUDE.md", 10))
        self.assertEqual(tier_of(fl, diff(".claude/tools/a.py", src)), "meta")

    def test_a_multi_crate_logic_diff_is_full(self):
        src = [f"if a{i} {{ b(); }}" for i in range(200)]
        fl = files(("sdk/rs/src/a.rs", 200), ("feeds/src/b.rs", 200))
        text = diff("sdk/rs/src/a.rs", src) + diff("feeds/src/b.rs", src)
        self.assertEqual(tier_of(fl, text, code_crates=2), "full")


class GateTests(unittest.TestCase):
    def gates(self, *paths, base=None, prose=False):
        fl = files(*[(p, 1) for p in paths])
        return rt.gates(fl, kinds_of(paths), base, prose)

    def test_a_program_handler_fires_security(self):
        g = self.gates("tui/src/ui.rs", "programs/dropset/src/swap.rs")
        self.assertEqual(g["security"], {"fires": True, "path": "programs/dropset/src/swap.rs"})

    def test_a_hooks_guard_fires_security(self):
        self.assertTrue(self.gates(".claude/hooks/guard.py")["security"]["fires"])

    def test_host_tooling_and_docs_do_not(self):
        g = self.gates("tui/src/ui.rs", ".claude/tools/x.py", "programs/README.md")
        self.assertFalse(g["security"]["fires"])

    def test_the_freshness_surface(self):
        for path in ("CLAUDE.md", "docs/conventions/x.md", ".github/x.yml", ".claude/skills/a/SKILL.md"):
            self.assertTrue(self.gates(path)["freshness"]["fires"], path)
        self.assertFalse(self.gates("docs/architecture.md")["freshness"]["fires"])

    def test_a_new_top_level_tree(self):
        g = self.gates("tui/a.rs", "vault/src/lib.rs", "Makefile", base={"tui"})
        self.assertEqual(g["new_tree"]["path"], "vault/src/lib.rs")
        self.assertFalse(self.gates("vault/a.rs")["new_tree"]["fires"])

    def test_prose_heavy_passes_through(self):
        self.assertTrue(self.gates("a.rs", prose=True)["prose_heavy"]["fires"])


class LensTests(unittest.TestCase):
    def fired(self, *names):
        g = {n: {"fires": n in names, "path": None} for n in rt.GATE_LENSES}
        return g

    def names(self, tier, *gates):
        return [e["lens"] for e in rt.lenses_for(tier, self.fired(*gates))]

    def test_every_tier_ends_in_a_required_cross_check(self):
        for tier in rt.TIER_LENSES:
            lenses = rt.lenses_for(tier, self.fired())
            self.assertEqual(lenses[-1]["lens"], "cross-check", tier)
            self.assertTrue(lenses[-1]["required"])

    def test_only_the_trivial_tier_may_ask_to_skip_it(self):
        for tier in rt.TIER_LENSES:
            skip = [e["lens"] for e in rt.lenses_for(tier, self.fired()) if e["ask_to_skip"]]
            self.assertEqual(skip, ["cross-check"] if tier == "trivial" else [], tier)

    def test_a_gate_only_adds(self):
        for tier in rt.TIER_LENSES:
            base = set(self.names(tier))
            for gate in rt.GATE_LENSES:
                self.assertLessEqual(base, set(self.names(tier, gate)), (tier, gate))

    def test_security_reaches_the_full_tier_through_its_gate(self):
        self.assertNotIn("security", self.names("full"))
        self.assertIn("security", self.names("full", "security"))

    def test_a_gate_names_itself_as_the_reason(self):
        lenses = rt.lenses_for("meta", self.fired("prose_heavy"))
        why = {e["lens"]: e["why"] for e in lenses}
        self.assertEqual(why["comment-accuracy"], "gate:prose_heavy")
        self.assertEqual(why["correctness"], "tier")

    def test_every_lens_named_has_a_brief_file(self):
        from pathlib import Path

        lens_dir = Path(__file__).resolve().parents[2] / "skills" / "review-pr" / "lenses"
        for name in rt.LENS_ORDER:
            self.assertTrue((lens_dir / f"{name}.md").is_file(), name)


class AbridgedAndSubdivisionTests(unittest.TestCase):
    def test_the_entry_gate_threshold(self):
        self.assertTrue(rt.abridged_eligible(files(("tui/a.rs", 60)), 1))
        self.assertFalse(rt.abridged_eligible(files(("tui/a.rs", 61)), 1))
        self.assertFalse(rt.abridged_eligible(files(*[(f"t/{i}", 1) for i in range(6)]), 1))
        self.assertFalse(rt.abridged_eligible(files(("sdk/ts/a.ts", 1)), 1))
        self.assertFalse(rt.abridged_eligible(files(("a/x", 1), ("b/y", 1)), 2))

    def test_subdivision_only_past_the_threshold_largest_first(self):
        fl = files(("sdk/rs/a.rs", 10), ("tui/a.rs", 50), ("Makefile", 5))
        self.assertIsNone(rt.subdivision(fl, rt.SUBDIVIDE_DIFF_LINES))
        groups = rt.subdivision(fl, rt.SUBDIVIDE_DIFF_LINES + 1)
        self.assertEqual(
            groups,
            [
                {"only": "tui/**", "changes": 50},
                {"only": "sdk/**", "changes": 10},
                {"only": "Makefile", "changes": 5},
            ],
        )


class IntegrityTests(unittest.TestCase):
    def kinds(self, text):
        diffs = rt.parse_diff(text)
        hits = rt.integrity_hits(diffs, kinds_of([d.path for d in diffs]))
        return [h["kind"] for h in hits]

    def test_a_clean_diff_has_no_hits(self):
        self.assertEqual(self.kinds(diff("tui/a.rs", ["let x = 1;"])), [])

    def test_a_removed_test(self):
        text = diff("tui/tests/a.rs", removed=["#[test]", "fn t() {}"])
        self.assertEqual(self.kinds(text), ["test-removed"])

    def test_a_renamed_test_is_not_removed(self):
        text = diff("tui/tests/a.rs", ["#[test]", "fn u() {}"], ["#[test]", "fn t() {}"])
        self.assertEqual(self.kinds(text), [])

    def test_each_added_line_kind(self):
        cases = {
            "test-skipped": '#[ignore = "slow"]',
            "placeholder": "todo!()",
            "suppression": "#[allow(dead_code)]",
            "gate-bypass": "continue-on-error: true",
            "swallowed-exception": "except Exception: pass",
        }
        for kind, line in cases.items():
            self.assertIn(kind, self.kinds(diff("tui/src/a.rs", [line])), kind)

    def test_prose_is_not_scanned_for_spellings(self):
        self.assertEqual(self.kinds(diff("docs/a.md", ["never pass --no-verify"])), [])

    def test_a_relaxed_timeout(self):
        text = diff("tui/src/a.rs", ["let timeout_ms = 5000;"], ["let timeout_ms = 500;"])
        self.assertEqual(self.kinds(text), ["relaxed-bound"])

    def test_an_exact_count_turned_into_a_range(self):
        text = diff(
            "tui/src/a.rs", ["assert!(rows.len() >= 3);"], ["assert_eq!(rows.len(), 3);"]
        )
        self.assertEqual(self.kinds(text), ["relaxed-assertion"])

    def test_a_snapshot_changed_without_a_test(self):
        text = diff("tui/src/snapshots/a.snap", ["frame"], ["old frame"])
        self.assertEqual(self.kinds(text), ["snapshot-without-test"])

    def test_the_summary_caps_hits_but_counts_all(self):
        hits = [{"kind": "placeholder"}] * (rt.INTEGRITY_SHOWN + 5)
        summary = rt.integrity_summary(hits)
        self.assertEqual(summary["count"], rt.INTEGRITY_SHOWN + 5)
        self.assertEqual(len(summary["hits"]), rt.INTEGRITY_SHOWN)
        self.assertEqual(summary["by_kind"], {"placeholder": rt.INTEGRITY_SHOWN + 5})


if __name__ == "__main__":
    unittest.main()
