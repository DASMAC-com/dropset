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


class BuildCmd(unittest.TestCase):
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
    def run_main(self, *argv: str) -> tuple[int, str]:
        out = io.StringIO()
        with mock.patch.object(cs, "tracked_rust_files", return_value=TRACKED):
            with redirect_stdout(out):
                code = cs.main(["--root", self.root, *argv])
        return code, out.getvalue().strip()

    def test_print_doc_scoped(self):
        self.assertEqual(
            self.run_main("doc", "--print", "util/src/lib.rs"),
            (0, "RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps -p util"),
        )

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


if __name__ == "__main__":
    unittest.main()
