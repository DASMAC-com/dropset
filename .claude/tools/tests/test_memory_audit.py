#!/usr/bin/env python3
"""Unit tests for `memory_audit.py` (stdlib unittest).

The fixture is a real on-disk memory store plus a real repo root, because every
finding this tool produces is a statement about the filesystem — a mock would only
assert that the module agrees with itself.

The assertion that matters most is the negative one: a healthy store produces no
findings. A staleness scanner that cries wolf is worse than no scanner, since the
step's autonomy bound makes a human confirm every candidate.
"""

from __future__ import annotations

import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import memory_audit  # noqa: E402
from memory_audit import (  # noqa: E402
    MemoryAuditError,
    audit,
    index_pointers,
    over_long_index_lines,
    path_candidates,
    render,
    run,
    slug_stem,
)


def _store(memories: dict[str, str], index: str | None = None) -> Path:
    """Write a memory store and return its directory."""
    root = Path(tempfile.mkdtemp())
    memory_dir = root / "memory"
    memory_dir.mkdir()
    for name, body in memories.items():
        (memory_dir / name).write_text(body, encoding="utf-8")
    if index is None:
        index = "\n".join(f"- [{Path(n).stem}]({n}) — hook" for n in sorted(memories))
    (memory_dir / "MEMORY.md").write_text(index + "\n", encoding="utf-8")
    return memory_dir


def _repo(paths: list[str]) -> Path:
    """A repo root containing exactly ``paths``."""
    root = Path(tempfile.mkdtemp())
    for rel in paths:
        target = root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("x", encoding="utf-8")
    return root


HEALTHY = {
    "eng-1-alpha.md": "Body citing `.claude/tools/real.py` and nothing else.\n",
    "beta.md": "Body citing `docs/conventions/real.md`.\n",
}
HEALTHY_REPO = [".claude/tools/real.py", "docs/conventions/real.md"]


class HealthyStore(unittest.TestCase):
    def test_a_healthy_store_produces_no_findings_at_all(self):
        result = audit(_store(HEALTHY), _repo(HEALTHY_REPO))
        self.assertEqual(result["findings"], [])
        self.assertEqual(result["over_long_index_lines"], 0)
        self.assertEqual(result["memories"], 2)

    def test_the_index_itself_is_never_audited_as_a_memory(self):
        # MEMORY.md is the index, not an entry; counting it would report a
        # permanent phantom desync on every pass.
        result = audit(_store(HEALTHY), _repo(HEALTHY_REPO))
        self.assertEqual(result["memories"], 2)
        self.assertNotIn("MEMORY", str(result["findings"]))

    def test_a_missing_store_is_an_error_not_an_empty_pass(self):
        with self.assertRaises(MemoryAuditError):
            audit(Path(tempfile.mkdtemp()) / "absent", _repo([]))


class IndexDesync(unittest.TestCase):
    def test_a_pointer_with_no_file_is_reported(self):
        memory_dir = _store(HEALTHY)
        (memory_dir / "MEMORY.md").write_text(
            "- [gone](gone.md) — hook\n", encoding="utf-8"
        )
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        kinds = [(f["kind"], f["slug"]) for f in result["findings"]]
        self.assertIn(("index-desync", "gone"), kinds)

    def test_a_file_with_no_pointer_is_reported(self):
        # The other half of a half-done purge, and the direction a scan that only
        # walked the index would miss entirely.
        memory_dir = _store(HEALTHY, index="- [beta](beta.md) — hook")
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        reasons = {f["slug"]: f["reason"] for f in result["findings"]}
        self.assertIn("eng-1-alpha", reasons)
        self.assertIn("no MEMORY.md pointer", reasons["eng-1-alpha"])

    def test_a_pointer_in_a_subdirectory_form_still_matches_by_name(self):
        memory_dir = _store(
            HEALTHY, index="- [beta](./beta.md) — h\n- [a](eng-1-alpha.md) — h"
        )
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        self.assertEqual(
            [f for f in result["findings"] if f["kind"] == "index-desync"], []
        )


class DanglingPaths(unittest.TestCase):
    def test_a_cited_path_absent_at_head_is_reported(self):
        memory_dir = _store({"m.md": "See `.claude/tools/retired.py` for detail.\n"})
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        dangling = [f for f in result["findings"] if f["kind"] == "dangling-path"]
        self.assertEqual(len(dangling), 1)
        self.assertIn("retired.py", dangling[0]["reason"])

    def test_a_glob_that_matches_nothing_is_dangling(self):
        memory_dir = _store({"m.md": "The family `.claude/tools/*.rs` is gone.\n"})
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        self.assertEqual([f["kind"] for f in result["findings"]], ["dangling-path"])

    def test_a_glob_that_matches_is_not_dangling(self):
        memory_dir = _store({"m.md": "The family `.claude/tools/*.py` still exists.\n"})
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        self.assertEqual(result["findings"], [])

    def test_many_missing_paths_are_summarized_not_listed_in_full(self):
        body = " ".join(f"`docs/gone{i}.md`" for i in range(9))
        memory_dir = _store({"m.md": body})
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        reason = result["findings"][0]["reason"]
        self.assertIn("9 path(s)", reason)
        self.assertIn("more", reason)
        # The point of the cap: the reason stays one line.
        self.assertNotIn("\n", reason)


#: Stands in for the repo's top-level entries.
TOP = frozenset({".claude", "docs", "frontend", "bench"})


class PathExtraction(unittest.TestCase):
    def test_urls_secrets_and_absolute_paths_are_not_repo_paths(self):
        # Each of these is a real reference that simply cannot be resolved against
        # the repo root, so reporting it would be a false alarm.
        body = (
            "`https://example.com/a/b` `op://vault/item/credential` "
            "`~/.claude/settings.json` `/etc/hosts`"
        )
        self.assertEqual(path_candidates(body, TOP), [])

    def test_prose_outside_backticks_is_ignored(self):
        self.assertEqual(path_candidates("see docs/conventions/thing.md now", TOP), [])

    def test_a_bare_word_in_backticks_is_not_a_path(self):
        self.assertEqual(path_candidates("the `--flag` and `cargo`", TOP), [])

    def test_trailing_punctuation_is_stripped(self):
        self.assertEqual(path_candidates("at `docs/a.md.`", TOP), ["docs/a.md"])

    def test_a_trailing_slash_directory_resolves(self):
        self.assertEqual(
            path_candidates("under `docs/conventions/`", TOP), ["docs/conventions"]
        )

    def test_slash_shaped_things_that_are_not_paths_are_excluded(self):
        # The regression that motivated anchoring on a real top-level entry: a
        # bare "contains a slash" test reported 25 candidates against the live
        # store, of which about 4 were real. Every string below came out of that
        # run, and none of their first segments is a repo entry.
        for token in (
            "EUR/USDT",  # a currency pair
            "AUD/USD",
            "10.0.0.0/16",  # a CIDR block
            "some-owner/paths-filter",  # a pinned GitHub Action ref
            "some-owner/rust-cache",
            "actions/cache/restore",
            "origin/main",  # a git ref
            "refs/pull/N/merge",
            "gh-readonly-queue/main/pr-343-x",
            "DASMAC-com/dropset",  # a repo slug
            "solana-foundation/anchor",
            "api.kraken.com/0/public",  # a scheme-less URL
            "dropset/oanda/api-key",  # a secret reference
            "src/lib.rs",  # crate-relative, unresolvable against the root
            "./migrations",
        ):
            with self.subTest(token=token):
                self.assertEqual(path_candidates(f"see `{token}` here", TOP), [])

    def test_a_real_repo_path_still_qualifies(self):
        # The anchor must not be so tight that it rejects what the check is for.
        for token in (
            ".claude/tools/ci_metrics.py",
            "docs/conventions/context-economy.md",
            "frontend/components/swap/RouteModeToggle.tsx",
            "bench/programs/prop-amm/anchor-v2",
        ):
            with self.subTest(token=token):
                self.assertEqual(path_candidates(f"see `{token}`", TOP), [token])

    def test_an_unknown_top_level_segment_is_skipped(self):
        self.assertEqual(path_candidates("`absent-tree/thing.md`", TOP), [])


class OverLongIndex(unittest.TestCase):
    def test_the_count_is_reported_and_only_the_worst_few_are_named(self):
        long_a = "- [a](a.md) — " + "x" * 300
        long_b = "- [b](b.md) — " + "y" * 200
        long_c = "- [c](c.md) — " + "z" * 180
        index = "\n".join([long_a, long_b, long_c, "- [d](d.md) — short"])
        memory_dir = _store({"a.md": "", "b.md": "", "c.md": "", "d.md": ""}, index)
        result = audit(memory_dir, _repo([]))
        self.assertEqual(result["over_long_index_lines"], 3)

        out = render(result, worst=2)
        row = next(line for line in out if line.startswith("over-long-index"))
        # Count first, then exactly the worst two — widest first, and the third
        # over-long line named nowhere. Reporting all of them was the waste.
        self.assertIn("3 MEMORY.md line(s)", row)
        self.assertIn("line 1", row)
        self.assertIn("line 2", row)
        self.assertNotIn("line 3", row)

    def test_headings_and_blanks_are_not_over_long_candidates(self):
        self.assertEqual(over_long_index_lines("# " + "x" * 300 + "\n\n", 160), [])

    def test_widest_first(self):
        rows = over_long_index_lines("a" * 200 + "\n" + "b" * 400 + "\n", 160)
        self.assertEqual([w for _, w in rows], [400, 200])


class SupersededHeuristic(unittest.TestCase):
    def test_two_memories_sharing_an_eng_stem_are_a_candidate(self):
        memory_dir = _store(
            {"eng-9-first.md": "a\n", "eng-9-second.md": "b\n", "other.md": "c\n"}
        )
        result = audit(memory_dir, _repo([]))
        found = [f for f in result["findings"] if f["kind"] == "superseded-candidate"]
        self.assertEqual(len(found), 1)
        self.assertIn("eng-9", found[0]["reason"])
        self.assertIn("heuristic", found[0]["reason"])

    def test_a_lone_eng_memory_is_not_a_candidate(self):
        memory_dir = _store({"eng-9-only.md": "a\n"})
        result = audit(memory_dir, _repo([]))
        self.assertEqual(
            [f for f in result["findings"] if f["kind"] == "superseded-candidate"], []
        )

    def test_a_generic_first_word_is_not_a_stem(self):
        # `feedback-*` memories share a topic, not a subject. Grouping on it
        # produced noise rather than candidates.
        self.assertIsNone(slug_stem("feedback-one-thing"))
        self.assertIsNone(slug_stem("ci-something"))
        self.assertEqual(slug_stem("eng-1194-thing"), "eng-1194")


class DisabledPathCheck(unittest.TestCase):
    """A path check that did not run must not read as a clean store.

    The wrong-root case does NOT raise: a readable directory that simply is not
    this repo has a perfectly good top-level listing, so every cited path is
    dropped for an unknown first segment and the findings list comes back empty.
    That is why the signal is the drop rate rather than the root itself.
    """

    def test_a_wrong_but_readable_root_is_announced(self):
        memory_dir = _store({"m.md": "cites `docs/conventions/thing.md`\n"})
        # A real directory that is not this repo — the measured failure shape.
        wrong = _repo(["unrelated/file.txt"])
        result = audit(memory_dir, wrong)
        self.assertEqual(result["paths_shaped"], 1)
        self.assertEqual(result["paths_anchored"], 0)
        text = "\n".join(render(result, worst=3))
        self.assertIn("path-check-DISABLED", text)

    def test_a_correct_root_is_not_announced(self):
        result = audit(_store(HEALTHY), _repo(HEALTHY_REPO))
        self.assertTrue(result["paths_anchored"])
        self.assertNotIn("DISABLED", "\n".join(render(result, worst=3)))

    def test_a_store_citing_no_paths_at_all_is_not_announced(self):
        # The other half: zero shaped candidates is a clean store, not a broken
        # check, and conflating them would make the warning permanent noise.
        result = audit(_store({"m.md": "prose with no paths\n"}), _repo(HEALTHY_REPO))
        self.assertEqual(result["paths_shaped"], 0)
        self.assertNotIn("DISABLED", "\n".join(render(result, worst=3)))


class BoundedReport(unittest.TestCase):
    def test_a_flood_of_one_kind_is_capped_and_counted(self):
        # The tool's own worst failure mode: a POINTER_RE regression makes EVERY
        # memory report "no pointer". Uncapped that is ~96 lines into the main
        # loop — more than the improvised shapes this tool replaced.
        memories = {f"m{i}.md": "body\n" for i in range(12)}
        memory_dir = _store(memories, index="- [none](none.md) — hook")
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        text = render(result, worst=3)
        desync = [line for line in text if line.startswith("index-desync:")]
        self.assertEqual(len(desync), 3)
        # The overflow marker must not share the finding prefix, or nothing
        # counting findings can tell them apart.
        self.assertTrue(
            any(line.startswith("-- +10 more index-desync") for line in text)
        )

    def test_the_summary_still_reports_the_true_total(self):
        memories = {f"m{i}.md": "body\n" for i in range(12)}
        memory_dir = _store(memories, index="- [none](none.md) — hook")
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        # 12 files with no pointer, plus the one dangling pointer.
        self.assertEqual(len(result["findings"]), 13)
        self.assertIn("13 finding(s)", render(result, worst=3)[-1])

    def test_json_mode_bounds_the_over_long_list(self):
        long_index = "\n".join(f"- [a{i}](a{i}.md) — " + "x" * 300 for i in range(10))
        memory_dir = _store({f"a{i}.md": "" for i in range(10)}, index=long_index)
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            run(
                [
                    "memory_audit.py",
                    str(memory_dir),
                    "--repo-root",
                    str(_repo(HEALTHY_REPO)),
                    "--json",
                    "--worst",
                    "3",
                ]
            )
        payload = json.loads(out.getvalue())
        self.assertEqual(payload["over_long_index_lines"], 10)
        self.assertEqual(len(payload["over_long_worst"]), 3)


class Reporting(unittest.TestCase):
    def test_no_memory_body_ever_reaches_the_report(self):
        # The whole reason the tool exists: the report enters the main loop.
        secret = "SENTINEL-BODY-TEXT-THAT-MUST-NOT-APPEAR"
        memory_dir = _store({"m.md": f"{secret}\nSee `docs/gone.md`.\n"})
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        text = "\n".join(render(result, worst=3))
        self.assertIn("dangling-path", text)
        self.assertNotIn(secret, text)

    def test_the_summary_line_names_the_breakdown(self):
        result = audit(_store(HEALTHY), _repo(HEALTHY_REPO))
        summary = render(result, worst=3)[-1]
        self.assertIn("2 memories", summary)
        self.assertIn("0 finding(s)", summary)
        self.assertIn("none", summary)


class Cli(unittest.TestCase):
    def _invoke(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = run(["memory_audit.py", *argv])
        return rc, out.getvalue(), err.getvalue()

    def test_a_clean_store_exits_zero_with_a_summary(self):
        memory_dir = _store(HEALTHY)
        repo = _repo(HEALTHY_REPO)
        rc, out, _ = self._invoke(str(memory_dir), "--repo-root", str(repo))
        self.assertEqual(rc, 0)
        self.assertIn("memory-audit |", out)

    def test_findings_do_not_make_it_exit_non_zero(self):
        # Candidates are input to a human decision, not a failure — and a
        # non-zero exit here would read as a broken pass in the report.
        memory_dir = _store({"m.md": "`docs/gone.md`\n"})
        repo = _repo(HEALTHY_REPO)
        rc, out, _ = self._invoke(str(memory_dir), "--repo-root", str(repo))
        self.assertEqual(rc, 0)
        self.assertIn("dangling-path", out)

    def test_json_mode_emits_the_raw_data(self):
        memory_dir = _store(HEALTHY)
        rc, out, _ = self._invoke(
            str(memory_dir), "--repo-root", str(_repo(HEALTHY_REPO)), "--json"
        )
        self.assertEqual(rc, 0)
        self.assertIn('"findings"', out)

    def test_a_missing_store_reports_an_error_and_exits_one(self):
        absent = str(Path(tempfile.mkdtemp()) / "absent")
        original = sys.argv
        sys.argv = ["memory_audit.py", absent]
        self.addCleanup(setattr, sys, "argv", original)
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = memory_audit.main()
        self.assertEqual(rc, 1)
        self.assertIn("no memory store", err.getvalue())

    def test_help_needs_no_store(self):
        rc, out, _ = self._invoke("--help")
        self.assertEqual(rc, 0)
        self.assertIn("Usage:", out)

    def test_bare_invocation_prints_help_rather_than_failing(self):
        rc, out, _ = self._invoke()
        self.assertEqual(rc, 0)
        self.assertIn("Usage:", out)

    def test_an_unknown_flag_is_refused(self):
        with self.assertRaises(MemoryAuditError):
            run(["memory_audit.py", "somewhere", "--nope"])


class IndexPointerParsing(unittest.TestCase):
    def test_pointers_are_read_in_order(self):
        text = "- [A](a.md) — x\n* [B](b.md) — y\nnot a pointer\n"
        self.assertEqual(index_pointers(text), ["a.md", "b.md"])

    def test_a_line_without_a_link_is_not_a_pointer(self):
        self.assertEqual(index_pointers("- just text\n"), [])

    def test_a_title_containing_brackets_still_parses(self):
        # Taken verbatim from the live index. A `[^\]]*` title class stops inside
        # the title here and the line reads as no pointer, which reported a false
        # index-desync for a memory that is indexed correctly.
        text = (
            "- [anchor v2 #[program] cfg limitation]"
            "(anchor-v2-program-cfg-limitation.md) — use a runtime guard\n"
        )
        self.assertEqual(index_pointers(text), ["anchor-v2-program-cfg-limitation.md"])

    def test_a_bracketed_title_memory_is_not_reported_as_a_desync(self):
        # The end-to-end form of the bug, so a regression shows up as a finding
        # rather than only as a parse difference.
        memory_dir = _store(
            {"anchor-v2-program-cfg-limitation.md": "body\n"},
            index=(
                "- [anchor v2 #[program] cfg limitation]"
                "(anchor-v2-program-cfg-limitation.md) — hook"
            ),
        )
        result = audit(memory_dir, _repo(HEALTHY_REPO))
        self.assertEqual(result["findings"], [])

    def test_a_non_md_link_in_the_hook_text_is_not_taken_as_the_target(self):
        text = "- [Title](thing.md) — see [docs](https://example.com/x)\n"
        self.assertEqual(index_pointers(text), ["thing.md"])

    def test_a_non_md_FIRST_link_does_not_latch_onto_a_later_one(self):
        # The permissive title class used to skip the first link entirely and
        # return `z.md` — the same wander-past-the-link family as the bracket bug.
        text = "- [T](x.png) — see [y](z.md)\n"
        self.assertEqual(index_pointers(text), [])

    def test_a_target_carrying_an_anchor_still_parses(self):
        # `(file.md#section)` used to fail the `.md)` requirement outright, which
        # reproduced exactly the false index-desync the bracket fix removed.
        text = "- [Title](thing.md#a-section) — hook\n"
        self.assertEqual(index_pointers(text), ["thing.md"])


if __name__ == "__main__":
    unittest.main()
