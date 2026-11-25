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
  [[ "${DS_POOL:-1}" == 0 ]]
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
    {"name": "DS_POOL", "kind": "toggle", "required": False, "default": "1"},
    {"name": "DS_WAIT", "kind": "knob", "required": False, "default": "120"},
    {"name": "LINEAR_KEY", "kind": "launcher-set", "required": False, "default": None},
]

EXAMPLE = (
    "DS_LANES=2\nDS_MODEL='<model-id>'\nDS_REF='op://<vault>/<item>/<field>'\n"
    "# DS_REGION=x\n# export DS_WAIT=120\n# DS_POOL=0\n"
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
        self.assertIn("7 rows, in sync", out)

    def test_the_scan_reads_code_lines_and_quoted_names_only(self):
        reads = rc.scan(self.root)
        self.assertEqual(
            set(reads.sites),
            {r["name"] for r in ROWS},
        )
        self.assertEqual(reads.defaults["DS_REGION"], {"us-west-2"})
        self.assertEqual(reads.defaults["DS_WAIT"], {"120"})

    def test_an_undocumented_read_fails(self):
        self.write_manifest([r for r in ROWS if r["name"] != "DS_WAIT"])
        code, out = self.call("--static")
        self.assertEqual(code, 1)
        self.assertIn("undocumented read: DS_WAIT (.claude/tools/pool.py:3)", out)

    def test_a_quoted_name_in_a_hook_is_a_read(self):
        self.put(".claude/hooks/guard.py", 'VAULT_ENV = "DS_VAULT"\n')
        code, out = self.call("--static")
        self.assertEqual(code, 1)
        self.assertIn("undocumented read: DS_VAULT (.claude/hooks/guard.py:1)", out)

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

    def test_a_tool_read_row_the_example_assigns_plainly_fails(self):
        # DS_WAIT is read by a tool, so only an export reaches it; DS_REGION is
        # read by the shell alone, so a plain assignment is fine; a per-command
        # prefix is no profile line.
        self.put(
            ".claude/shell/runtime-config.example.zsh",
            EXAMPLE.replace("# export DS_WAIT=120", "DS_WAIT=120")
            + "# DS_WAIT=1 plan\n",
        )
        code, out = self.call("--static")
        self.assertEqual(code, 1)
        self.assertEqual(out.count("export gap:"), 1, out)
        self.assertIn("export gap: DS_WAIT", out)

    def test_a_malformed_manifest_exits_2(self):
        for rows in (
            [dict(ROWS[0], kind="secret")],
            [dict(ROWS[0], default="2")],  # required with a default
            [ROWS[0], ROWS[0]],
            # An opaque default would print beside a pinned value.
            [dict(ROWS[3], default="op://v/i/f")],
            # `required_unless` only qualifies a required row, holds strings
            # (the environment's type) and names a row.
            [dict(ROWS[1], required_unless={"DS_REGION": "0"})],
            [dict(ROWS[0], required_unless={"DS_REGION": 0}), ROWS[1]],
            [dict(ROWS[0], required_unless={"DS_NOWHERE": "0"})],
        ):
            with self.subTest(rows=rows):
                self.write_manifest(rows)
                self.assertEqual(self.call("--static")[0], 2)

    def test_a_maps_to_row_matching_shell_and_script_is_in_sync(self):
        self.map_wait('  export ITERM_WAIT="${DS_WAIT:-120}"\n', "${ITERM_WAIT:-120}")
        code, out = self.call("--static")
        self.assertEqual(code, 0, out)

    def test_a_maps_to_script_default_disagreeing_fails(self):
        self.map_wait('  export ITERM_WAIT="${DS_WAIT:-120}"\n', "${ITERM_WAIT:-30}")
        code, out = self.call("--static")
        self.assertEqual(code, 1)
        self.assertIn(
            "mapping drift: DS_WAIT manifest '120', "
            ".claude/scripts/wait.sh ITERM_WAIT ['30']",
            out,
        )

    def test_a_maps_to_the_shell_never_hands_over_fails(self):
        # Named only in a comment, which hands nothing over.
        self.map_wait("  # ITERM_WAIT=${DS_WAIT}\n", "${ITERM_WAIT:-120}")
        out = self.call("--static")[1]
        self.assertIn(
            "mapping gap: no shell init line hands DS_WAIT to ITERM_WAIT", out
        )

    def test_a_line_naming_both_without_assigning_is_no_hand_over(self):
        # Mentions both names, and a longer DS name, but assigns neither.
        self.map_wait(
            '  echo "$ITERM_WAIT ${DS_WAITER:-1} ${DS_WAIT}"\n', "${ITERM_WAIT:-120}"
        )
        out = self.call("--static")[1]
        self.assertIn(
            "mapping gap: no shell init line hands DS_WAIT to ITERM_WAIT", out
        )

    def test_a_maps_to_file_that_does_not_exist_fails(self):
        self.map_wait('  export ITERM_WAIT="${DS_WAIT:-120}"\n', "${ITERM_WAIT:-120}")
        (self.root / ".claude/scripts/wait.sh").unlink()
        out = self.call("--static")[1]
        self.assertIn(
            "mapping gap: DS_WAIT maps to .claude/scripts/wait.sh, which does not exist",
            out,
        )

    def test_a_maps_to_script_without_a_default_fails(self):
        self.map_wait('  export ITERM_WAIT="${DS_WAIT:-120}"\n', "$ITERM_WAIT")
        out = self.call("--static")[1]
        self.assertIn("mapping gap: .claude/scripts/wait.sh applies no default", out)

    def test_a_malformed_maps_to_exits_2(self):
        for row in (
            dict(ROWS[5], maps_to={"name": "ITERM_WAIT"}),  # no file
            dict(ROWS[5], maps_to="ITERM_WAIT"),
            dict(ROWS[2], maps_to={"file": "x.sh", "name": "Y"}),  # not a knob
            dict(ROWS[5], maps_to={"file": "", "name": "ITERM_WAIT"}),  # empty
            dict(ROWS[5], maps_to={"file": "x.sh", "name": "Y", "z": "1"}),  # extra key
            # A toggle with no default could never pass the check.
            dict(ROWS[4], default=None, maps_to={"file": "x.sh", "name": "Y"}),
        ):
            with self.subTest(row=row):
                self.write_manifest(
                    [row if r["name"] == row["name"] else r for r in ROWS]
                )
                self.assertEqual(self.call("--static")[0], 2)

    def map_wait(self, shell_line: str, script_default: str) -> None:
        """Map DS_WAIT onto a script's ITERM_WAIT, via one init.zsh line."""
        self.put(
            ".claude/shell/init.zsh", SHELL + "_ds_hand() {\n" + shell_line + "}\n"
        )
        self.put(".claude/scripts/wait.sh", f'WAIT="{script_default}"\n')
        maps_to = {"file": ".claude/scripts/wait.sh", "name": "ITERM_WAIT"}
        self.write_manifest(
            [dict(r, maps_to=maps_to) if r["name"] == "DS_WAIT" else r for r in ROWS]
        )

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
        # DS_LANES, DS_MODEL set; DS_REGION, DS_POOL, DS_WAIT defaulted;
        # DS_REF unset.
        self.assertEqual(line, "config: 5 of 6 rows set or defaulted")

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

    def test_an_unexported_tool_read_row_counts_as_missing(self):
        # DS_WAIT is optional but a tool reads it; DS_LANES is required but
        # only the shell reads it, so its plain assignment is fine.
        env = {**self.ENV, rc.UNEXPORTED_ENV: "DS_WAIT DS_LANES"}
        code, out = self.call("--live", "--json", env=env)
        self.assertEqual(code, 1)
        data = json.loads(out)
        self.assertEqual(data["missing"], ["DS_WAIT"])
        self.assertIn("UNEXPORTED DS_WAIT", data["rows"])
        self.assertIn("DS_LANES=2", data["rows"])
        self.assertTrue(
            data["line"].startswith("config: 1 assigned but not exported (DS_WAIT)"),
            data["line"],
        )

    def test_names_lists_the_rows_the_config_sets(self):
        code, out = self.call("--names")
        self.assertEqual(code, 0)
        self.assertEqual(
            out.split(),
            ["DS_LANES", "DS_REGION", "DS_MODEL", "DS_REF", "DS_POOL", "DS_WAIT"],
        )


if __name__ == "__main__":
    unittest.main()
