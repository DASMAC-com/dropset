"""Stdlib ``unittest`` tests for ``ci_mirror_check.py``.

One embedded fixture tree (two queue workflows, a pull-request-only one, a
manifest, a Makefile and a ruleset mirror) that is in sync, then one mutation
per failure kind, each asserting the finding names what drifted. The last
test runs the check over the committed tree. Run via ``make tools-tests``.
"""

from __future__ import annotations

import contextlib
import io
import json
import unittest
from pathlib import Path

import ci_mirror_check as cmc

ROOT = Path(__file__).resolve().parents[3]

LINT_YML = """\
---
jobs:
  lint:
    name: 'Lint'
    steps:
    - uses: 'actions/checkout@v6'
    # A comment naming `make decks-build` is not a run.
    - name: 'Tools tests'
      run: 'make tools-tests'
  lint-rust:
    name: 'Lint (${{ matrix.hook }})'
    steps:
    - run: 'pre-commit run clippy'
name: 'Lint'
'on':
  merge_group: null
  pull_request: null
"""

TEST_YML = """\
---
jobs:
  test:
    name: 'Tests'
    steps:
    - run: 'make program-keypair'
    - name: 'Verify IDL'
      run: |
        make idl
        git diff --exit-code
    - run: 'cargo build -p dropset
            --features wasm'
name: 'Tests'
'on':
  merge_group: null
  pull_request: null
"""

AUDIT_YML = """\
---
jobs:
  audit:
    name: 'Audit'
    steps:
    - run: 'make icon-audit'
name: 'Audit'
'on':
  pull_request: null
"""

MAKEFILE = """\
VAR := value
.PHONY: lint
lint:
\t@echo lint
tools-tests:
\t@echo tools
program-keypair:
\t@echo key
idl test: program-keypair
\t@echo idl
icon-audit:
\t@echo audit
"""

DOC = """\
# GitHub via MCP

### Required status checks

These contexts gate `main`, via a ruleset:

`Lint`, `Lint (clippy)`,
`Tests`.

`Lint (clippy)` is a matrix leg.

## Next
"""


def row(context, workflow, ci=(), local=()):
    return {
        "context": context,
        "workflow": workflow,
        "ci": list(ci),
        "local": list(local),
    }


def manifest(*rows):
    return json.dumps({"contexts": list(rows)})


ROWS = [
    row("Lint", "lint.yml", ["tools-tests"], ["lint", "tools-tests"]),
    row("Lint (clippy)", "lint.yml", [], ["lint"]),
    row("Tests", "test.yml", ["program-keypair", "idl"], ["test"]),
]


def run(workflows=None, rows=None, makefile=MAKEFILE, doc=DOC):
    if workflows is None:
        workflows = {"lint.yml": LINT_YML, "test.yml": TEST_YML, "audit.yml": AUDIT_YML}
    return cmc.check(
        workflows, manifest(*(ROWS if rows is None else rows)), makefile, doc
    )


class ParseTest(unittest.TestCase):
    def test_workflow_walk(self):
        wf = cmc.parse_workflow(TEST_YML)
        self.assertTrue(wf.gates_queue)
        self.assertEqual(wf.jobs["test"].name, "Tests")
        self.assertEqual(wf.jobs["test"].targets, ["program-keypair", "idl"])
        self.assertFalse(cmc.parse_workflow(AUDIT_YML).gates_queue)
        self.assertEqual(
            cmc.parse_workflow(LINT_YML).jobs["lint"].targets, ["tools-tests"]
        )

    def test_make_targets(self):
        for command, targets in (
            ("make -j4 a b && make c", ["a", "b", "c"]),
            ("make X=1 a; cmake b", ["a"]),
            ("# make a", []),
            ("if [ x ]; then make a; fi", ["a"]),
            ("FOO=1 make a", ["a"]),
            ("timeout 600 make a", ["a"]),
            ("make -C dir a", ["a"]),
            ("make -j 4 a", ["a"]),
            ("make a  # make b", ["a"]),
            ('echo "step #1" && make a', ["a"]),
        ):
            with self.subTest(command=command):
                self.assertEqual(cmc.make_targets(command), (targets, []))

    def test_unreadable_make_arguments(self):
        for command, arg in (
            ('make "$T"', '"$T"'),
            ("make ${{ matrix.t }}", "${{"),
            ("make \\", "\\"),
            ("make", "(no target)"),
        ):
            with self.subTest(command=command):
                self.assertEqual(cmc.make_targets(command), ([], [arg]))

    def test_trailing_comments_hide_nothing(self):
        wf = cmc.parse_workflow(
            "jobs:\n"
            "  a:  # first job\n"
            "    name: 'A'  # its context\n"
            "    steps:\n"
            "    - run: 'make tools-tests'  # mirrors make lint\n"
            "    - run: 'test ''a'' = a && make sdk'\n"
            '    - run: "test \\"a\\" = a && make wasm"  # escaped\n'
            "'on':\n"
            "  merge_group: null\n"
        )
        self.assertEqual(wf.jobs["a"].name, "A")
        self.assertEqual(wf.jobs["a"].targets, ["tools-tests", "sdk", "wasm"])

    def test_makefile_targets(self):
        self.assertEqual(
            cmc.makefile_targets(MAKEFILE),
            {"lint", "tools-tests", "program-keypair", "idl", "test", "icon-audit"},
        )

    def test_required_contexts(self):
        self.assertEqual(cmc.required_contexts(DOC), ["Lint", "Lint (clippy)", "Tests"])
        self.assertIsNone(cmc.required_contexts("# no such heading\n"))


class CheckTest(unittest.TestCase):
    def assertOneFinding(self, problems, *needles):
        self.assertEqual(len(problems), 1, problems)
        for needle in needles:
            self.assertIn(needle, problems[0])

    def test_in_sync_fixture_is_clean(self):
        # audit.yml runs `make icon-audit` with no row: it never gates the queue.
        self.assertEqual(run(), [])

    def test_a_ci_target_with_no_row(self):
        rows = [
            ROWS[0],
            ROWS[1],
            row("Tests", "test.yml", ["program-keypair"], ["test"]),
        ]
        self.assertOneFinding(run(rows=rows), "test.yml", "'Tests'", "`make idl`")

    def test_an_unreadable_make_argument(self):
        workflows = {
            "lint.yml": LINT_YML.replace(
                "run: 'make tools-tests'", "run: 'make tools-tests; make \"$T\"'"
            ),
            "test.yml": TEST_YML,
        }
        self.assertOneFinding(run(workflows=workflows), "'Lint'", '"$T"', "cannot read")

    def test_a_row_ci_target_the_workflow_no_longer_runs(self):
        rows = [
            row("Lint", "lint.yml", ["tools-tests", "decks-build"], ["lint"]),
            *ROWS[1:],
        ]
        makefile = MAKEFILE + "decks-build:\n\t@echo decks\n"
        self.assertOneFinding(
            run(rows=rows, makefile=makefile),
            "'Lint'",
            "`decks-build`",
            "no longer runs",
        )

    def test_a_deleted_row_is_named(self):
        # The row's context goes unmirrored, and its job's targets unlisted.
        problems = run(rows=ROWS[:2])
        self.assertIn(
            "required context 'Tests' (docs/conventions/github-mcp.md) has no "
            "cfg/ci-mirror.json row",
            problems,
        )
        self.assertEqual(len(problems), 3, problems)

    def test_a_row_for_an_unlisted_context(self):
        rows = [*ROWS, row("Extra", "lint.yml")]
        self.assertOneFinding(run(rows=rows), "'Extra'", "not a required context")

    def test_a_row_naming_a_non_queue_workflow(self):
        rows = [*ROWS[:2], row("Tests", "audit.yml", [], ["test"])]
        problems = run(rows=rows)
        self.assertTrue(
            any("audit.yml" in p and "merge_group" in p for p in problems), problems
        )

    def test_a_target_the_makefile_lacks(self):
        rows = [
            *ROWS[:2],
            row("Tests", "test.yml", ["program-keypair", "idl"], ["test-gone"]),
        ]
        self.assertOneFinding(
            run(rows=rows), "'Tests'", "`local`", "`test-gone`", "Makefile"
        )

    def test_a_missing_context_list(self):
        self.assertOneFinding(run(doc="# nothing here\n"), "no code-span context list")

    def test_a_malformed_manifest(self):
        problems = cmc.check({}, "not json", MAKEFILE, DOC)
        self.assertOneFinding(problems, "ci-mirror.json")


class CommittedTreeTest(unittest.TestCase):
    def test_the_committed_manifest_is_in_sync(self):
        problems, summary = cmc.check_tree(ROOT)
        self.assertEqual(problems, [])
        self.assertIn("11 rows", summary)

    def test_main_exits_zero_on_the_committed_tree(self):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(cmc.main(["--root", str(ROOT)]), 0)
        self.assertTrue(out.getvalue().startswith("ci-mirror-check: 11 rows"))


if __name__ == "__main__":
    unittest.main()
