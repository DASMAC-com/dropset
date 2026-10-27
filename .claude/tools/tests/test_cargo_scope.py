#!/usr/bin/env python3
"""Unit tests for ``cargo_scope.py`` (stdlib ``unittest``; no pytest).

A fixture workspace in a temp directory covers the path-to-package mapping
(nested packages, a workspace-level file, a non-member package, a non-Rust
file), the ``-p`` versus whole-workspace decision, and the argv built for
each hook.
"""

from __future__ import annotations

import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest import mock

import cargo_scope as cs

ROOT_MANIFEST = """\
[workspace]
members = ["util", "sdk/*"]
"""

TREE = {
    "Cargo.toml": ROOT_MANIFEST,
    "util/Cargo.toml": '[package]\nname = "util"\n',
    "util/src/lib.rs": "",
    "util/src/deep/mod.rs": "",
    "sdk/rs/Cargo.toml": '[package]\nname = "dropset-sdk"\n',
    "sdk/rs/src/lib.rs": "",
    # A package nested inside another package's tree owns its own files.
    "sdk/rs/inner/Cargo.toml": '[package]\nname = "inner"\n',
    "sdk/rs/inner/src/lib.rs": "",
    # A manifest with no `[package]` (a workspace-ish one) is walked past.
    "sdk/rs/src/gen/Cargo.toml": "[features]\n",
    "sdk/rs/src/gen/mod.rs": "",
    # Not a workspace member.
    "scratch/Cargo.toml": '[package]\nname = "scratch"\n',
    "scratch/src/main.rs": "",
    "build_helper.rs": "",
    "README.md": "",
}

TRACKED = {p for p in TREE if p.endswith(".rs")}


class Fixture(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name
        for rel, body in TREE.items():
            path = os.path.join(self.root, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as handle:
                handle.write(body)

    def tearDown(self):
        self._tmp.cleanup()

    def scope(self, *files: str):
        return cs.packages_for(list(files), self.root, tracked=TRACKED)


class OwningPackage(Fixture):
    def test_nearest_package_manifest(self):
        self.assertEqual(
            cs.owning_package("util/src/deep/mod.rs", self.root),
            ("util", "util"),
        )

    def test_nested_package_wins_over_outer(self):
        self.assertEqual(
            cs.owning_package("sdk/rs/inner/src/lib.rs", self.root),
            ("sdk/rs/inner", "inner"),
        )

    def test_manifest_without_package_is_walked_past(self):
        self.assertEqual(
            cs.owning_package("sdk/rs/src/gen/mod.rs", self.root),
            ("sdk/rs", "dropset-sdk"),
        )

    def test_workspace_level_file_has_no_owner(self):
        self.assertIsNone(cs.owning_package("build_helper.rs", self.root))


class WorkspaceMembers(Fixture):
    def test_globs_expand(self):
        self.assertEqual(cs.workspace_members(self.root), {"util", "sdk/rs"})


class PackagesFor(Fixture):
    def test_one_crate(self):
        self.assertEqual(self.scope("util/src/lib.rs"), ["util"])

    def test_several_crates_sorted_and_deduped(self):
        self.assertEqual(
            self.scope(
                "util/src/lib.rs",
                "sdk/rs/src/lib.rs",
                "util/src/deep/mod.rs",
            ),
            ["dropset-sdk", "util"],
        )

    def test_non_rust_files_are_ignored(self):
        self.assertEqual(self.scope("README.md", "util/src/lib.rs"), ["util"])

    def test_no_rust_file_runs_nothing(self):
        self.assertEqual(self.scope("README.md", "Cargo.toml"), [])

    def test_every_tracked_file_is_the_workspace(self):
        self.assertIsNone(self.scope(*TRACKED, "README.md"))

    def test_superset_of_tracked_is_the_workspace(self):
        # Local `make lint` adds untracked files to the tracked set.
        self.assertIsNone(self.scope(*TRACKED, "util/src/new.rs"))

    def test_workspace_level_file_falls_back_to_workspace(self):
        self.assertIsNone(self.scope("util/src/lib.rs", "build_helper.rs"))

    def test_non_member_package_falls_back_to_workspace(self):
        self.assertIsNone(self.scope("scratch/src/main.rs"))

    def test_nested_package_outside_members_falls_back(self):
        # `sdk/rs/inner` is not listed (the glob is one level), so cargo
        # would refuse `-p inner`.
        self.assertIsNone(self.scope("sdk/rs/inner/src/lib.rs"))

    def test_dot_slash_paths_normalize(self):
        self.assertEqual(self.scope("./util/src/lib.rs"), ["util"])

    def test_path_outside_root_falls_back_without_walking(self):
        # An absolute path once walked up to `/` forever.
        self.assertIsNone(self.scope("/elsewhere/src/lib.rs"))
        self.assertIsNone(self.scope("../elsewhere/src/lib.rs"))


class TrackedRustFiles(Fixture):
    def test_drops_tracked_files_deleted_from_disk(self):
        os.remove(os.path.join(self.root, "util/src/deep/mod.rs"))
        listed = "\0".join(sorted(TRACKED)) + "\0"
        with mock.patch.object(
            cs.subprocess, "run", return_value=mock.Mock(stdout=listed)
        ):
            tracked = cs.tracked_rust_files(self.root)
        self.assertEqual(tracked, TRACKED - {"util/src/deep/mod.rs"})


METADATA = {
    "workspace_members": ["feeds-id", "util-id"],
    "packages": [
        {"id": "feeds-id", "name": "dropset-feeds"},
        {"id": "util-id", "name": "util"},
        {"id": "serde-id", "name": "serde"},
    ],
    "resolve": {
        "nodes": [
            {"id": "feeds-id", "features": ["store", "http", "default"]},
            {"id": "util-id", "features": []},
            {"id": "serde-id", "features": ["derive", "std"]},
        ]
    },
}


class ResolvedFeatures(unittest.TestCase):
    def test_members_only_sorted_default_dropped(self):
        self.assertEqual(
            cs.parse_resolved_features(METADATA),
            {"dropset-feeds": ["http", "store"], "util": []},
        )

    def test_failed_metadata_is_none(self):
        with mock.patch.object(
            cs.subprocess, "run", return_value=mock.Mock(returncode=101)
        ):
            self.assertIsNone(cs.resolved_features("."))


class BuildCmd(unittest.TestCase):
    def test_features_ride_each_selected_crate(self):
        self.assertEqual(
            cs.build_cmd(
                "clippy",
                ["dropset-feeds", "util"],
                {"dropset-feeds": ["http", "store"], "util": [], "other": ["x"]},
            ),
            [
                "cargo",
                "clippy",
                "-p",
                "dropset-feeds",
                "-p",
                "util",
                "--features",
                "dropset-feeds/http,dropset-feeds/store",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        )

    def test_empty_package_list_is_refused(self):
        # `[]` means nothing to run; it must never become the workspace.
        with self.assertRaises(ValueError):
            cs.build_cmd("clippy", [])

    def test_clippy_scoped(self):
        self.assertEqual(
            cs.build_cmd("clippy", ["a", "b"]),
            [
                "cargo",
                "clippy",
                "-p",
                "a",
                "-p",
                "b",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        )

    def test_doc_scoped(self):
        self.assertEqual(
            cs.build_cmd("doc", ["a"]),
            ["cargo", "doc", "--no-deps", "-p", "a"],
        )

    def test_workspace_forms_match_the_old_hook_entries(self):
        self.assertEqual(
            cs.build_cmd("clippy", None),
            "cargo clippy --all-targets -- -D warnings".split(),
        )
        self.assertEqual(
            cs.build_cmd("doc", None),
            "cargo doc --no-deps --workspace".split(),
        )

    def test_doc_env(self):
        self.assertEqual(cs.build_env("doc"), {"RUSTDOCFLAGS": "-Dwarnings"})
        self.assertEqual(cs.build_env("clippy"), {})


class Main(Fixture):
    def run_main(self, *argv: str, features=None) -> tuple[int, str]:
        out = io.StringIO()
        with (
            mock.patch.object(cs, "tracked_rust_files", return_value=TRACKED),
            mock.patch.object(
                cs, "resolved_features", return_value=features or {"util": []}
            ),
            redirect_stdout(out),
        ):
            code = cs.main(["--root", self.root, *argv])
        return code, out.getvalue().strip()

    def test_print_doc_scoped(self):
        self.assertEqual(
            self.run_main("doc", "--print", "util/src/lib.rs"),
            (0, "RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps -p util"),
        )

    def test_print_passes_the_workspace_features(self):
        code, shown = self.run_main(
            "clippy", "--print", "util/src/lib.rs", features={"util": ["std"]}
        )
        self.assertEqual(code, 0)
        self.assertIn("-p util --features util/std --all-targets", shown)

    def test_metadata_failure_falls_back_to_the_workspace(self):
        out = io.StringIO()
        with (
            mock.patch.object(cs, "tracked_rust_files", return_value=TRACKED),
            mock.patch.object(cs, "resolved_features", return_value=None),
            redirect_stdout(out),
        ):
            cs.main(["--root", self.root, "clippy", "--print", "util/src/lib.rs"])
        self.assertEqual(
            out.getvalue().strip(), "cargo clippy --all-targets -- -D warnings"
        )

    def test_absolute_path_inside_root_is_made_relative(self):
        path = os.path.join(self.root, "util/src/lib.rs")
        code, shown = self.run_main("doc", "--print", path)
        self.assertEqual(code, 0)
        self.assertTrue(shown.endswith("cargo doc --no-deps -p util"))

    def test_empty_set_runs_nothing(self):
        with mock.patch.object(cs.subprocess, "run") as run:
            code, _ = self.run_main("clippy", "README.md")
        self.assertEqual(code, 0)
        run.assert_not_called()

    def test_runs_cargo_with_env_and_returns_its_code(self):
        result = mock.Mock(returncode=101)
        with mock.patch.object(cs.subprocess, "run", return_value=result) as run:
            with mock.patch("sys.stderr", new=io.StringIO()):
                code, _ = self.run_main("doc", "util/src/lib.rs")
        self.assertEqual(code, 101)
        cmd = run.call_args.args[0]
        self.assertEqual(cmd, ["cargo", "doc", "--no-deps", "-p", "util"])
        self.assertEqual(run.call_args.kwargs["env"]["RUSTDOCFLAGS"], "-Dwarnings")


def _node(pkg_id: str, *deps: str, features=()) -> dict:
    return {
        "id": pkg_id,
        "features": list(features),
        "deps": [{"pkg": dep} for dep in deps],
    }


# util ← sdk ← leaf, util ← dropset (the program, dev-depending on sdk too),
# and feeds standing alone; serde is a non-member every crate uses.
GRAPH_METADATA = {
    "workspace_members": ["util", "sdk", "leaf", "dropset", "feeds"],
    "packages": [
        {"id": pkg_id, "name": pkg_id}
        for pkg_id in ("util", "sdk", "leaf", "dropset", "feeds", "serde")
    ],
    "resolve": {
        "nodes": [
            _node("util", "serde"),
            _node("sdk", "util", "serde"),
            _node("leaf", "sdk"),
            _node("dropset", "util", "sdk", features=["default", "asm"]),
            _node("feeds", "serde", features=["http"]),
            _node("serde"),
        ]
    },
}

GRAPH_TREE = {
    "Cargo.toml": '[workspace]\nmembers = ["util", "sdk", "leaf", '
    '"programs/dropset", "feeds"]\n',
    "Cargo.lock": "",
    "util/Cargo.toml": '[package]\nname = "util"\n',
    "util/src/lib.rs": "",
    "sdk/Cargo.toml": '[package]\nname = "sdk"\n',
    "sdk/src/lib.rs": "",
    "leaf/Cargo.toml": '[package]\nname = "leaf"\n',
    "leaf/src/lib.rs": "",
    "leaf/src/config.rs": "",
    "leaf/README.md": "",
    "programs/dropset/Cargo.toml": '[package]\nname = "dropset"\n',
    "programs/dropset/src/lib.rs": "",
    "programs/dropset/src/asm/entry.s": "",
    "feeds/Cargo.toml": '[package]\nname = "feeds"\n',
    "feeds/src/lib.rs": "",
    "docs/ci.md": "",
    "frontend/roster.json": "",
}

GRAPH_TRACKED = {p for p in GRAPH_TREE if p.endswith(".rs")}

# A feeds test reads the frontend roster at runtime, and another `include_str!`s
# leaf's source, though feeds does not depend on leaf (market-data reads the
# maker-bot's `config.rs` this way). An sdk test reads a vectors directory; a
# non-member's test reads a data file.
GRAPH_FIXTURES = {
    "frontend/roster.json": ["feeds/tests/roster.rs"],
    "leaf/src/config.rs": ["feeds/tests/config_agreement.rs"],
    "vectors": ["sdk/tests/replay.rs"],
    "scratch.json": ["scratch/tests/read.rs"],
}


class GraphFixture(Fixture):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name
        for rel, body in GRAPH_TREE.items():
            path = os.path.join(self.root, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as handle:
                handle.write(body)

    def seeds(self, *files: str):
        return cs.test_seeds(
            list(files), self.root, tracked=GRAPH_TRACKED, fixtures=GRAPH_FIXTURES
        )


class AffectedClosure(unittest.TestCase):
    def test_reverse_edges_are_members_only(self):
        dependents = cs.reverse_dependents(GRAPH_METADATA)
        self.assertEqual(dependents["util"], {"sdk", "dropset"})
        self.assertEqual(dependents["sdk"], {"leaf", "dropset"})
        self.assertNotIn("serde", dependents)

    def test_leaf_is_only_itself(self):
        self.assertEqual(cs.affected_closure(["leaf"], GRAPH_METADATA), ["leaf"])

    def test_shared_dependency_takes_every_dependent_transitively(self):
        self.assertEqual(
            cs.affected_closure(["util"], GRAPH_METADATA),
            ["dropset", "leaf", "sdk", "util"],
        )

    def test_program_pulls_in_its_artifact_consumers(self):
        with mock.patch.dict(cs.ARTIFACT_CONSUMERS, {"dropset": ("dropset", "feeds")}):
            self.assertEqual(
                cs.affected_closure(["dropset"], GRAPH_METADATA), ["dropset", "feeds"]
            )

    def test_every_member_is_the_workspace(self):
        self.assertIs(
            cs.affected_closure(["util", "feeds"], GRAPH_METADATA), cs.WORKSPACE
        )


class TestSeeds(GraphFixture):
    def test_no_rust_reachable_change_tests_nothing(self):
        self.assertEqual(self.seeds("docs/ci.md", "leaf/README.md"), [])

    def test_non_rust_build_input_seeds_its_crate(self):
        self.assertEqual(self.seeds("programs/dropset/src/asm/entry.s"), ["dropset"])

    def test_fixture_seeds_the_reading_crate(self):
        self.assertEqual(self.seeds("frontend/roster.json"), ["feeds"])

    def test_unowned_build_input_is_the_workspace(self):
        self.assertIs(self.seeds("Cargo.lock"), cs.WORKSPACE)

    def test_every_tracked_file_is_the_workspace(self):
        self.assertIs(self.seeds(*GRAPH_TRACKED), cs.WORKSPACE)

    def test_rust_source_read_as_a_fixture_seeds_the_reader(self):
        self.assertEqual(self.seeds("leaf/src/config.rs"), ["feeds", "leaf"])

    def test_rust_fixture_found_without_a_non_rust_path(self):
        # The scan once ran only on reaching a non-`.rs` path, so a branch
        # changing only Rust missed the cross-crate reader.
        with mock.patch.object(
            cs.review_diff, "rust_test_fixtures", return_value=GRAPH_FIXTURES
        ) as scan:
            seeds = cs.test_seeds(
                ["leaf/src/config.rs"], self.root, tracked=GRAPH_TRACKED
            )
        self.assertEqual(seeds, ["feeds", "leaf"])
        scan.assert_called_once()

    def test_program_build_input_seeds_the_program(self):
        self.assertEqual(self.seeds("Anchor.toml"), ["dropset"])

    def test_fixture_directory_key_matches_files_under_it(self):
        self.assertEqual(self.seeds("vectors/swap/01.json"), ["sdk"])

    def test_non_member_fixture_reader_is_the_workspace(self):
        self.assertIs(self.seeds("scratch.json"), cs.WORKSPACE)

    def test_non_member_build_input_is_the_workspace(self):
        self.assertIs(self.seeds("scratch/src/main.rs"), cs.WORKSPACE)


class TestCommands(unittest.TestCase):
    def test_program_in_the_set_builds_it_first(self):
        self.assertEqual(cs.artifact_builds(["dropset", "sdk"]), [["make", "program"]])
        self.assertEqual(cs.artifact_builds(cs.WORKSPACE), [["make", "program"]])
        self.assertEqual(cs.artifact_builds(["leaf"]), [])

    def test_scoped_test_carries_the_workspace_features(self):
        self.assertEqual(
            cs.build_cmd("test", ["feeds"], {"feeds": ["http"]}),
            ["cargo", "test", "-p", "feeds", "--features", "feeds/http"],
        )

    def test_workspace_test_matches_make_test(self):
        self.assertEqual(cs.build_cmd("test", cs.WORKSPACE), ["cargo", "test"])

    def test_scope_flags(self):
        self.assertEqual(cs.scope_flags(["a", "b"]), "-p a -p b")
        self.assertEqual(cs.scope_flags(cs.WORKSPACE), "--workspace")


class TestMain(GraphFixture):
    def run_main(self, *argv: str, metadata=GRAPH_METADATA) -> tuple[int, str]:
        out = io.StringIO()
        with (
            mock.patch.object(cs, "tracked_rust_files", return_value=GRAPH_TRACKED),
            mock.patch.object(
                cs.review_diff, "rust_test_fixtures", return_value=GRAPH_FIXTURES
            ),
            mock.patch.object(cs, "cargo_metadata", return_value=metadata) as meta,
            mock.patch("sys.stderr", new=io.StringIO()),
            redirect_stdout(out),
        ):
            code = cs.main(["--root", self.root, *argv])
        self.meta = meta
        return code, out.getvalue().strip()

    def test_print_program_change(self):
        self.assertEqual(
            self.run_main("test", "--print", "programs/dropset/src/lib.rs"),
            (0, "make program\ncargo test -p dropset --features dropset/asm"),
        )

    def test_affected_prints_the_flags(self):
        self.assertEqual(self.run_main("affected", "leaf/src/lib.rs"), (0, "-p leaf"))

    def test_affected_widens_the_seeds(self):
        self.assertEqual(
            self.run_main("affected", "sdk/src/lib.rs"),
            (0, "-p dropset -p leaf -p sdk"),
        )

    def test_affected_without_metadata_is_the_workspace(self):
        self.assertEqual(
            self.run_main("affected", "leaf/src/lib.rs", metadata=None),
            (0, "--workspace"),
        )

    def test_affected_with_nothing_to_test_prints_nothing(self):
        self.assertEqual(self.run_main("affected", "docs/ci.md"), (0, ""))
        self.meta.assert_not_called()

    def test_changed_adds_the_branch_files(self):
        with mock.patch.object(cs, "changed_paths", return_value=["sdk/src/lib.rs"]):
            shown = self.run_main("affected", "--changed", "feeds/src/lib.rs")
        self.assertEqual(shown, (0, "-p dropset -p feeds -p leaf -p sdk"))

    def test_docs_only_change_never_asks_cargo(self):
        with mock.patch.object(cs.subprocess, "run") as run:
            code, shown = self.run_main("test", "docs/ci.md")
        self.assertEqual((code, shown), (0, ""))
        run.assert_not_called()
        self.meta.assert_not_called()

    def test_metadata_failure_runs_the_full_suite(self):
        self.assertEqual(
            self.run_main("test", "--print", "leaf/src/lib.rs", metadata=None),
            (0, "make program\ncargo test"),
        )

    def test_stops_at_the_first_failing_command(self):
        result = mock.Mock(returncode=2)
        with mock.patch.object(cs.subprocess, "run", return_value=result) as run:
            code, _ = self.run_main("test", "programs/dropset/src/lib.rs")
        self.assertEqual(code, 2)
        self.assertEqual(run.call_args.args[0], ["make", "program"])


class ChangedPaths(unittest.TestCase):
    def test_keeps_deletions_and_untracked_without_renames(self):
        outputs = [
            mock.Mock(stdout="util/src/gone.rs\0sdk/src/lib.rs\0"),
            mock.Mock(stdout="leaf/src/new.rs\0"),
        ]
        with (
            mock.patch.object(cs.lint_paths, "merge_base", return_value="abc"),
            mock.patch.object(cs.subprocess, "run", side_effect=outputs) as run,
        ):
            paths = cs.changed_paths("/nonexistent")
        self.assertEqual(
            sorted(paths), ["leaf/src/new.rs", "sdk/src/lib.rs", "util/src/gone.rs"]
        )
        self.assertIn("--no-renames", run.call_args_list[0].args[0])

    def test_empty_diff_is_only_the_untracked(self):
        outputs = [mock.Mock(stdout=""), mock.Mock(stdout="leaf/src/new.rs\0")]
        with (
            mock.patch.object(cs.lint_paths, "merge_base", return_value="abc"),
            mock.patch.object(cs.subprocess, "run", side_effect=outputs),
        ):
            self.assertEqual(cs.changed_paths("/nonexistent"), ["leaf/src/new.rs"])


if __name__ == "__main__":
    unittest.main()
