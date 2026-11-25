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
            rt.parse_diff(text),
            {"tui/src/keys.rs": "source"},
            {"tui/src/keys.rs": [(8, 12)]},
        )
        self.assertEqual(shape["hunks"], 1)

    def test_a_source_hunk_outside_the_test_range_is_not(self):
        text = diff("src/lib.rs", ["fn a() {}"], start=2)
        shape = rt.test_shape(
            rt.parse_diff(text), {"src/lib.rs": "source"}, {"src/lib.rs": [(8, 12)]}
        )
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
        text = diff(
            "programs/dropset/src/a.rs", ["const X: u8 = 2;"], ["const X: u8 = 1;"]
        )
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

    def test_indented_code_is_reduced_reformat(self):
        old = [f"let v{i} = {i};" for i in range(40)]
        new = [f"    let v{i} = {i};" for i in range(40)]
        fl = files(("tui/src/a.rs", 80))
        self.assertEqual(
            tier_of(fl, diff("tui/src/a.rs", new, old)), "reduced-reformat"
        )

    def test_value_swaps_are_reduced_rewiring(self):
        old = [f"const C{i}: u64 = {i};" for i in range(40)]
        new = [f"const C{i}: u64 = {i * 2 + 100};" for i in range(40)]
        fl = files(("tui/src/a.rs", 80))
        self.assertEqual(
            tier_of(fl, diff("tui/src/a.rs", new, old)), "reduced-rewiring"
        )

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
        self.assertEqual(
            g["security"], {"fires": True, "path": "programs/dropset/src/swap.rs"}
        )

    def test_a_hooks_guard_fires_security(self):
        self.assertTrue(self.gates(".claude/hooks/guard.py")["security"]["fires"])

    def test_host_tooling_and_docs_do_not(self):
        g = self.gates("tui/src/ui.rs", ".claude/tools/x.py", "programs/README.md")
        self.assertFalse(g["security"]["fires"])

    def test_the_freshness_surface(self):
        for path in (
            "CLAUDE.md",
            "docs/conventions/x.md",
            ".github/x.yml",
            ".claude/skills/a/SKILL.md",
        ):
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
            skip = [
                e["lens"] for e in rt.lenses_for(tier, self.fired()) if e["ask_to_skip"]
            ]
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

        lens_dir = (
            Path(__file__).resolve().parents[2] / "skills" / "review-pr" / "lenses"
        )
        for name in rt.LENS_ORDER:
            self.assertTrue((lens_dir / f"{name}.md").is_file(), name)


class AbridgedAndSubdivisionTests(unittest.TestCase):
    def test_the_entry_gate_threshold(self):
        self.assertTrue(rt.abridged_eligible(files(("tui/a.rs", 60)), 1))
        self.assertFalse(rt.abridged_eligible(files(("tui/a.rs", 61)), 1))
        self.assertFalse(
            rt.abridged_eligible(files(*[(f"t/{i}", 1) for i in range(6)]), 1)
        )
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

    def test_a_dominant_tree_beside_a_small_one_still_splits_deeper(self):
        # This PR's own shape: `.claude/**` held 2,929 of 2,931 changes.
        fl = files(
            (".claude/tools/a.py", 900), (".claude/skills/x.md", 800), ("cfg/b.json", 2)
        )
        groups = rt.subdivision(fl, rt.SUBDIVIDE_DIFF_LINES + 1)
        self.assertEqual(
            [g["only"] for g in groups],
            [".claude/tools/**", ".claude/skills/**", "cfg/b.json"],
        )

    def test_one_shared_tree_subdivides_a_level_deeper(self):
        fl = files((".claude/tools/a.py", 30), (".claude/skills/x/SKILL.md", 9))
        groups = rt.subdivision(fl, rt.SUBDIVIDE_DIFF_LINES + 1)
        self.assertEqual(
            [g["only"] for g in groups], [".claude/tools/**", ".claude/skills/**"]
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
        text = diff(
            "tui/tests/a.rs", ["#[test]", "fn u() {}"], ["#[test]", "fn t() {}"]
        )
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
        text = diff(
            "tui/src/a.rs", ["let timeout_ms = 5000;"], ["let timeout_ms = 500;"]
        )
        self.assertEqual(self.kinds(text), ["relaxed-bound"])

    def test_an_exact_count_turned_into_a_range(self):
        text = diff(
            "tui/src/a.rs",
            ["assert!(rows.len() >= 3);"],
            ["assert_eq!(rows.len(), 3);"],
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


class UnderTieringRegressionTests(unittest.TestCase):
    """The review's own probes: each once tiered a real change too narrowly."""

    def test_new_logic_around_a_few_indented_lines_is_not_a_move(self):
        moved = [f"value_{i} = compute(x, {i})" for i in range(20)]
        new = [f"if guard_{i}(x): return grant(x, {i})" for i in range(300)]
        fl = files(("bots/src/a.py", 340))
        text = diff("bots/src/a.py", ["    " + m for m in moved] + new, moved)
        self.assertNotEqual(tier_of(fl, text, code_crates=1), "reduced-move")

    def test_filler_lines_alone_do_not_make_a_move(self):
        old = ["    }", "    )", "    Ok(())", "    return"] * 10
        new = ["    }", "    )", "    Ok(())", "    return"] * 10
        self.assertEqual(
            rt.moved_share(
                rt.parse_diff(diff("tui/src/a.rs", new, old)),
                {"tui/src/a.rs": "source"},
            ),
            0.0,
        )

    def test_deleting_a_guard_is_not_trivial(self):
        fl = files(("feeds/src/auth.rs", 2))
        text = diff(
            "feeds/src/auth.rs",
            removed=["if !authorized(u) {", "    return Err(Denied);"],
        )
        self.assertNotEqual(tier_of(fl, text), "trivial")

    def test_a_dependency_bump_with_its_lockfile_is_not_trivial(self):
        reviewed = files(("Cargo.toml", 2))
        inventory = reviewed + files(("Cargo.lock", 40))
        tier = rt.classify_tier(
            reviewed,
            kinds_of(["Cargo.toml"]),
            rt.parse_diff(
                diff("Cargo.toml", ['serde = "1.0.200"'], ['serde = "1.0.199"'])
            ),
            {},
            1,
            inventory=inventory,
        )
        self.assertNotEqual(tier["name"], "trivial")

    def test_a_root_manifest_fires_the_security_gate(self):
        g = rt.gates(files(("Cargo.toml", 2)), kinds_of(["Cargo.toml"]), None, False)
        self.assertTrue(g["security"]["fires"])

    def test_a_python_dedent_is_not_a_reformat(self):
        old = [f"        total += item_{i}" for i in range(25)]
        new = [f"    total += item_{i}" for i in range(25)]
        fl = files((".claude/tools/a.py", 50))
        self.assertFalse(
            rt.is_reformat(
                rt.parse_diff(diff(".claude/tools/a.py", new, old)),
                {".claude/tools/a.py": "source"},
            )
        )
        self.assertNotEqual(
            tier_of(fl, diff(".claude/tools/a.py", new, old)), "reduced-reformat"
        )

    def test_an_arithmetic_edit_is_not_rewiring(self):
        old = [f"let fee_{i} = a * b / c;" for i in range(40)]
        new = [f"let fee_{i} = a * b / d;" for i in range(40)]
        fl = files(("bots/src/fee.rs", 80))
        self.assertNotEqual(
            tier_of(fl, diff("bots/src/fee.rs", new, old)), "reduced-rewiring"
        )

    def test_a_type_edit_between_lifetimes_is_not_rewiring(self):
        # No `for`: the control-flow check would block rewiring on its own and
        # leave the skeleton untested.
        old = [f"type View{i}<'a> = Foo<'a, u8>;" for i in range(40)]
        new = [f"type View{i}<'a> = Baz<'a, u8>;" for i in range(40)]
        fl = files(("bots/src/a.rs", 80))
        self.assertNotEqual(
            tier_of(fl, diff("bots/src/a.rs", new, old)), "reduced-rewiring"
        )

    def test_a_char_literal_swap_is_still_rewiring(self):
        old = [f"const SEP_{i}: char = 'a';" for i in range(40)]
        new = [f"const SEP_{i}: char = '\\n';" for i in range(40)]
        fl = files(("bots/src/a.rs", 80))
        self.assertEqual(
            tier_of(fl, diff("bots/src/a.rs", new, old)), "reduced-rewiring"
        )

    def test_one_id_mirrored_across_languages_is_a_mirrored_literal(self):
        # Double quotes in the TS copy too: the literal pattern reads `'…'` as
        # a Rust char literal only.
        lib, ts = "programs/dropset/src/lib.rs", "sdk/ts/src/id.ts"
        text = diff(lib, ['declare_id!("NewId");'], ['declare_id!("OldId");'])
        text += diff(ts, ['export const ID = "NewId";'], ['export const ID = "OldId";'])
        diffs = rt.parse_diff(text)
        kinds = kinds_of([d.path for d in diffs])
        self.assertTrue(rt.mirrored_literal(diffs, kinds))

    def test_two_different_swaps_are_not_a_mirrored_literal(self):
        old = [f"const C{i}: u64 = {i};" for i in range(4)]
        new = [f"const C{i}: u64 = {i + 100};" for i in range(4)]
        diffs = rt.parse_diff(diff("bots/src/a.rs", new, old))
        self.assertTrue(rt.is_rewiring(diffs, {"bots/src/a.rs": "source"}))
        self.assertFalse(rt.mirrored_literal(diffs, {"bots/src/a.rs": "source"}))

    def test_one_test_line_beside_new_logic_is_not_small_single_crate(self):
        src = [f"if a{i} {{ b(); }}" for i in range(200)]
        fl = files(("tui/src/a.rs", 200), ("tui/tests/a.rs", 2))
        text = diff("tui/src/a.rs", src) + diff(
            "tui/tests/a.rs", ["assert!(x);"], ["assert!(y);"]
        )
        self.assertNotEqual(tier_of(fl, text), "small-single-crate")

    def test_a_production_change_with_no_hunk_blocks_test_only(self):
        added = [f"    if x {{ assert!(y{i}); }}" for i in range(40)]
        fl = files(("tui/tests/a.rs", 40), ("tui/run.sh", 0))
        text = diff("tui/tests/a.rs", added) + (
            "diff --git a/tui/run.sh b/tui/run.sh\nold mode 100644\nnew mode 100755\n"
        )
        self.assertNotEqual(tier_of(fl, text), "reduced-test-only")

    def test_an_added_build_manifest_fires_the_new_tree_gate(self):
        text = diff(
            "tui/vault/Cargo.toml", ["[package]", 'name = "vault"'], new_file=True
        )
        (fd,) = rt.parse_diff(text)
        self.assertTrue(fd.new_file)
        g = rt.gates(
            files(("tui/vault/Cargo.toml", 2)),
            kinds_of(["tui/vault/Cargo.toml"]),
            {"tui"},
            False,
            {"tui/vault/Cargo.toml"},
        )
        self.assertEqual(g["new_tree"]["path"], "tui/vault/Cargo.toml")


class ParseDiffEdgeTests(unittest.TestCase):
    def test_a_form_feed_in_a_changed_line_keeps_the_numbering(self):
        text = "diff --git a/x.c b/x.c\n--- a/x.c\n+++ b/x.c\n@@ -1,0 +1,2 @@\n+a\x0cb\n+c\n"
        (fd,) = rt.parse_diff(text)
        self.assertEqual(fd.hunks[0].added, [(1, "a\x0cb"), (2, "c")])

    def test_a_quoted_header_is_recognized_and_unescaped(self):
        self.assertEqual(
            rt.diff_header_path('diff --git "a/caf\\303\\251.md" "b/caf\\303\\251.md"'),
            "café.md",
        )

    def test_letter_escapes_are_unescaped(self):
        self.assertEqual(rt.unquote_c_path('a\\"b\\\\c\\td'), 'a"b\\c\td')

    def test_an_octal_control_byte_is_unescaped(self):
        self.assertEqual(rt.unquote_c_path("a\\001b"), "a\x01b")

    def test_an_unknown_escape_is_kept(self):
        self.assertEqual(rt.unquote_c_path("a\\qb"), "a\\qb")

    def test_a_quoted_path_reaches_the_kind_lookup(self):
        path = "tui/tests/café.rs"
        text = (
            'diff --git "a/tui/tests/caf\\303\\251.rs" "b/tui/tests/caf\\303\\251.rs"\n'
            "@@ -1,1 +1,0 @@\n-assert!(ok);\n"
        )
        (fd,) = rt.parse_diff(text)
        self.assertEqual(fd.path, path)
        hits = rt.integrity_hits([fd], kinds_of([path]))
        self.assertEqual([h["kind"] for h in hits], ["assertion-removed"])


class SelfMatchTest(unittest.TestCase):
    """The tagger's own pattern definitions raise no hit on a diff of its file."""

    PATH = ".claude/tools/review_tier.py"

    def hits_for(self, text):
        return rt.integrity_hits(rt.parse_diff(text), {self.PATH: "source"})

    def lines(self):
        import inspect

        return inspect.getsource(rt).split("\n")

    def test_review_tier_source_tags_nothing_when_added(self):
        self.assertEqual(
            self.hits_for(diff(self.PATH, self.lines(), new_file=True)), []
        )

    def test_review_tier_source_tags_nothing_when_rewritten(self):
        # One hunk removing and re-adding every line exercises the
        # removed-line and relaxation rules the added-only case cannot.
        lines = self.lines()
        self.assertEqual(self.hits_for(diff(self.PATH, lines, lines)), [])


class IntegrityAdditionsTests(unittest.TestCase):
    def kinds(self, text, ranges=None):
        diffs = rt.parse_diff(text)
        return [
            h["kind"]
            for h in rt.integrity_hits(diffs, kinds_of([d.path for d in diffs]), ranges)
        ]

    def test_a_deleted_assertion_in_a_kept_test(self):
        text = diff(
            "tui/tests/a.rs", ["let x = f();"], ["let x = f();", "assert_eq!(x, 3);"]
        )
        self.assertEqual(self.kinds(text), ["assertion-removed"])

    def test_a_deleted_assertion_in_an_inline_rust_test(self):
        text = diff("tui/src/a.rs", removed=["        assert!(ok);"], start=20)
        self.assertIn(
            "assertion-removed", self.kinds(text, {"tui/src/a.rs": [(10, 40)]})
        )

    def hits(self, text):
        diffs = rt.parse_diff(text)
        return rt.integrity_hits(diffs, kinds_of([d.path for d in diffs]))

    def test_the_quoted_assertion_is_the_one_dropped(self):
        # The first assertion is lost; the second is only re-valued, so it
        # pairs with its replacement by skeleton.
        text = diff(
            "tui/tests/a.rs",
            ["assert_eq!(y, 5);"],
            ["assert!(x.is_ok());", "assert_eq!(y, 4);"],
        )
        hits = [h for h in self.hits(text) if h["kind"] == "assertion-removed"]
        self.assertEqual([h["text"] for h in hits], ["assert!(x.is_ok());"])

    def test_an_exact_match_pairs_before_a_skeleton_match(self):
        # `y, 5` survives verbatim, so `y, 4` is the one lost.
        text = diff(
            "tui/tests/a.rs",
            ["assert_eq!(y, 5);"],
            ["assert_eq!(y, 4);", "assert_eq!(y, 5);"],
        )
        hits = [h for h in self.hits(text) if h["kind"] == "assertion-removed"]
        self.assertEqual([h["text"] for h in hits], ["assert_eq!(y, 4);"])

    def test_a_moved_assertion_is_not_a_hit(self):
        text = diff(
            "tui/tests/a.rs",
            ["let z = 1;", "assert!(ok);"],
            ["assert!(ok);", "let z = 1;"],
        )
        self.assertNotIn("assertion-removed", self.kinds(text))

    def test_a_rewritten_assertion_still_nets_out(self):
        text = diff("tui/tests/a.rs", ["assert!(x.is_some());"], ["assert!(ok);"])
        self.assertNotIn("assertion-removed", self.kinds(text))

    def test_a_rust_expect_is_not_an_assertion(self):
        text = diff("tui/tests/a.rs", removed=['let v = f().expect("v");'])
        self.assertNotIn("assertion-removed", self.kinds(text))

    def test_gate_bypass_spellings(self):
        for line in (
            "git commit --no-verify",
            "msg = '[skip ci]'",
            "run: make || true",
        ):
            self.assertIn("gate-bypass", self.kinds(diff("tui/src/a.sh", [line])), line)

    def test_a_two_line_swallowed_exception(self):
        text = diff("tui/a.py", ["try:", "    f()", "except Exception:", "    pass"])
        self.assertIn("swallowed-exception", self.kinds(text))

    def test_relaxed_retry_and_tolerance(self):
        for old, new in (
            ("max_retries = 3", "max_retries = 9"),
            ("tolerance = 0.01", "tolerance = 0.5"),
        ):
            self.assertIn(
                "relaxed-bound", self.kinds(diff("tui/a.py", [new], [old])), new
            )


if __name__ == "__main__":
    unittest.main()
