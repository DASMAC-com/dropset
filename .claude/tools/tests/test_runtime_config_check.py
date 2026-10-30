#!/usr/bin/env python3
"""Unit tests for ``runtime_config_check.py`` (stdlib ``unittest``; no pytest).

The load-bearing properties: static mode fails BOTH ways (an undocumented read
and a stale row), and on a knob default that disagrees with the code; live
mode never prints an opaque value and leads its line with a missing required
row. Each case builds a small embedded fixture tree in a temp directory, so
nothing depends on the real manifest; one case runs the real tree, the check
CI makes.
"""

from __future__ import annotations

import io
import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import runtime_config_check as rc

SHELL = """\
# DS_COMMENT_ONLY is named in a comment, which is not a read.
_ds_pool() {
  lanes="${DS_LANES-}"
  region="${DS_REGION:-us-west-2}"
  [[ -n "$DS_MODEL" ]] || return 1
  op read "$DS_REF"
}
"""

TOOL = """\
import os
TARGET_RE = r"CARGO_NOT_A_READ=(\\S+)"  # a longer literal is not a name
wait = knob("DS_WAIT", 120, float)
key = os.environ.get("LINEAR_KEY")
"""

ROWS = [
    {
        "name": "DS_LANES",
        "kind": "knob",
        "required": True,
        "default": None,
        "required_unless": {"DS_POOL": "0"},
    },
    {"name": "DS_REGION", "kind": "knob", "required": False, "default": "us-west-2"},
    {"name": "DS_MODEL", "kind": "model-id", "required": True, "default": None},
    {"name": "DS_REF", "kind": "secret-ref", "required": False, "default": None},
    {"name": "DS_WAIT", "kind": "knob", "required": False, "default": "120"},
    {"name": "LINEAR_KEY", "kind": "launcher-set", "required": False, "default": None},
]

EXAMPLE = (
    "DS_LANES=2\nDS_MODEL='<model-id>'\nDS_REF='op://<vault>/<item>/<field>'\n"
    "# DS_REGION=x\n# DS_WAIT=120\n"
)


class Fixture(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.put(".claude/shell/init.zsh", SHELL)
        self.put(".claude/tools/pool.py", TOOL)
        # Tests are never scanned: a fixture name there reads nothing.
        self.put(".claude/tools/tests/test_pool.py", 'env = {"DS_TEST_ONLY": "1"}\n')
        self.put(".claude/shell/runtime-config.example.zsh", EXAMPLE)
        self.write_manifest(ROWS)

    def tearDown(self):
        self._tmp.cleanup()

    def put(self, rel: str, text: str) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def write_manifest(self, rows, external=None) -> None:
        data = {"external": external or {}, "variables": rows}
        self.put(str(rc.MANIFEST), json.dumps(data))

    def call(self, *argv, env=None):
        out = io.StringIO()
        with redirect_stdout(out), redirect_stderr(io.StringIO()):
            code = rc.main(list(argv), env=env, root=self.root)
        return code, out.getvalue()


class StaticTests(Fixture):
    def test_a_manifest_matching_the_code_is_in_sync(self):
        code, out = self.call("--static")
        self.assertEqual(code, 0, out)
        self.assertIn("6 rows, in sync", out)

    def test_the_scan_reads_code_lines_and_quoted_names_only(self):
        reads = rc.scan(self.root)
        self.assertEqual(
            set(reads.sites),
            {"DS_LANES", "DS_REGION", "DS_MODEL", "DS_REF", "DS_WAIT", "LINEAR_KEY"},
        )
        self.assertEqual(reads.defaults["DS_REGION"], {"us-west-2"})
        self.assertEqual(reads.defaults["DS_WAIT"], {"120"})

    def test_an_undocumented_read_fails(self):
        self.write_manifest([r for r in ROWS if r["name"] != "DS_WAIT"])
        code, out = self.call("--static")
        self.assertEqual(code, 1)
        self.assertIn("undocumented read: DS_WAIT (.claude/tools/pool.py:3)", out)

    def test_a_stale_row_fails(self):
        self.write_manifest(ROWS + [dict(ROWS[0], name="DS_GONE")])
        code, out = self.call("--static")
        self.assertEqual(code, 1)
        self.assertIn("stale row: DS_GONE is read nowhere", out)

    def test_an_external_name_is_documented_but_must_still_be_read(self):
        self.put(".claude/tools/cargo.py", 'home = env.get("CARGO_HOME")\n')
        self.write_manifest(ROWS, external={"CARGO_HOME": "cargo's own"})
        self.assertEqual(self.call("--static")[0], 0)
        self.write_manifest(ROWS, external={"CARGO_HOME": "x", "CARGO_OLD": "y"})
        self.assertIn("stale row: CARGO_OLD", self.call("--static")[1])

    def test_a_knob_default_disagreeing_with_the_code_fails(self):
        rows = [dict(r, default="60") if r["name"] == "DS_WAIT" else r for r in ROWS]
        self.write_manifest(rows)
        code, out = self.call("--static")
        self.assertEqual(code, 1)
        self.assertIn("default drift: DS_WAIT manifest '60', code ['120']", out)

    def test_a_code_default_the_manifest_omits_fails(self):
        rows = [dict(r, default=None) if r["name"] == "DS_REGION" else r for r in ROWS]
        self.write_manifest(rows)
        self.assertIn("default drift: DS_REGION has none", self.call("--static")[1])

    def test_a_config_row_missing_from_the_example_fails(self):
        self.put(".claude/shell/runtime-config.example.zsh", "DS_LANES=2\n")
        out = self.call("--static")[1]
        self.assertIn("example gap: DS_MODEL", out)
        # A launcher-set row is never the config's to set.
        self.assertNotIn("LINEAR_KEY", out)

    def test_a_malformed_manifest_exits_2(self):
        for rows in (
            [dict(ROWS[0], kind="secret")],
            [dict(ROWS[0], default="2")],  # required with a default
            [ROWS[0], ROWS[0]],
            # An opaque default would print beside a pinned value.
            [dict(ROWS[3], default="op://v/i/f")],
            # `required_unless` only qualifies a required row.
            [dict(ROWS[1], required_unless={"DS_POOL": "0"})],
        ):
            with self.subTest(rows=rows):
                self.write_manifest(rows)
                self.assertEqual(self.call("--static")[0], 2)

    def test_the_real_tree_is_in_sync(self):
        # The check CI makes, so a drifting edit fails here too.
        out = io.StringIO()
        with redirect_stdout(out):
            code = rc.main(["--static"])
        self.assertEqual(code, 0, out.getvalue())


class LiveTests(Fixture):
    ENV = {
        "DS_LANES": "2",
        "DS_MODEL": "claude-secret-model-1",
        "DS_REF": "op://secret-vault/item/field",
        "DS_WAIT": "300",
        "LINEAR_KEY": "lin_api_secret",
        "DS_STRAY": "1",
        # Cargo's own configuration is never an unknown row.
        "CARGO_INCREMENTAL": "0",
        "HOME": "/Users/me",
    }

    def test_rows_report_set_default_and_pinned(self):
        code, out = self.call("--live", env=self.ENV)
        self.assertEqual(code, 0, out)
        self.assertIn("  DS_LANES=2", out)
        self.assertIn("  DS_MODEL set", out)
        self.assertIn("  DS_REGION at default (us-west-2)", out)
        self.assertIn("  pinned: DS_WAIT=300 (default 120)", out)
        self.assertIn("  DS_REF set", out)
        self.assertIn("  unknown: DS_STRAY", out)
        self.assertNotIn("CARGO_INCREMENTAL", out)

    def test_an_opaque_value_is_never_printed(self):
        for argv in (["--live"], ["--live", "--json"]):
            with self.subTest(argv=argv):
                _, out = self.call(*argv, env=self.ENV)
                self.assertNotIn("claude-secret-model-1", out)
                self.assertNotIn("secret-vault", out)

    def test_the_summary_counts_an_unset_optional_row_as_neither(self):
        env = {"DS_LANES": "2", "DS_MODEL": "m"}
        line = json.loads(self.call("--live", "--json", env=env)[1])["line"]
        # DS_LANES, DS_MODEL set; DS_REGION, DS_WAIT defaulted; DS_REF unset.
        self.assertEqual(line, "config: 4 of 5 rows set or defaulted")

    def test_required_unless_lifts_the_requirement(self):
        env = {k: v for k, v in self.ENV.items() if k != "DS_LANES"}
        code, out = self.call("--live", env={**env, "DS_POOL": "0"})
        self.assertEqual(code, 0, out)
        self.assertIn("  DS_LANES unset", out)

    def test_a_missing_required_row_leads_the_line_and_exits_1(self):
        env = {k: v for k, v in self.ENV.items() if k != "DS_LANES"}
        code, out = self.call("--live", "--json", env=env)
        self.assertEqual(code, 1)
        data = json.loads(out)
        self.assertEqual(data["missing"], ["DS_LANES"])
        self.assertTrue(
            data["line"].startswith("config: 1 required missing (DS_LANES)")
        )
        self.assertIn("MISSING DS_LANES", data["rows"])

    def test_names_lists_the_rows_the_config_sets(self):
        code, out = self.call("--names")
        self.assertEqual(code, 0)
        self.assertEqual(
            out.split(), ["DS_LANES", "DS_REGION", "DS_MODEL", "DS_REF", "DS_WAIT"]
        )


if __name__ == "__main__":
    unittest.main()
