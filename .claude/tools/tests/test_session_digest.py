"""Stdlib ``unittest`` tests for ``session_digest.py``, the ``digest``
subcommand of ``session_metrics.py``.

Run via the repo's ``make tools-tests``. Every fixture is a temporary projects
tree and base checkout; nothing reads the real Claude home.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

import session_digest as sd
import session_metrics as sm

MODEL = "claude-opus-5-5"
NOW = "2026-10-24T12:00:00Z"
SINCE = "2026-10-23T12:00:00Z"
# Inside the window and over an hour before `NOW`, so not live by age.
WINDOW_MTIME = 1_792_821_600  # 2026-10-24T06:00Z


def record(kind: str, stamp: str, cwd: str, **extra) -> dict:
    return {"type": kind, "timestamp": stamp, "cwd": cwd, **extra}


def assistant(msg_id: str, stamp: str, cwd: str, usage: dict, text: str = "") -> dict:
    content = [{"type": "text", "text": text}] if text else []
    return record(
        "assistant",
        stamp,
        cwd,
        message={"id": msg_id, "model": MODEL, "usage": usage, "content": content},
    )


def prompt(stamp: str, cwd: str, text: str) -> dict:
    return record("user", stamp, cwd, message={"role": "user", "content": text})


WRITE = {"cache_creation_input_tokens": 1_000_000, "output_tokens": 0}
READ = {"cache_read_input_tokens": 1_000_000, "output_tokens": 0}


class DigestFixture(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        self.base = root / "repo" / "dropset"
        (self.base / ".claude").mkdir(parents=True)
        self.projects = root / "projects"
        self.worktree = f"{self.base}/.claude/worktrees/2170"
        self.substrate_dir = self.base / sm.SUBSTRATE_DIR

    def write(self, cwd: str, session: str, records: list[dict], mtime=None) -> Path:
        slug = sm.slugify(Path(cwd))
        path = self.projects / slug / f"{session}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(json.dumps(r) + "\n" for r in records))
        if mtime is not None:
            os.utime(path, (mtime, mtime))
        return path

    def mark(self, session: str, substrate: str) -> None:
        sessions = self.substrate_dir / "sessions"
        sessions.mkdir(parents=True, exist_ok=True)
        (sessions / session).write_text(substrate + "\n")

    def run_digest(self, *extra: str) -> dict:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = sm.main(
                [
                    "digest",
                    "--base",
                    str(self.base),
                    "--projects",
                    str(self.projects),
                    "--now",
                    NOW,
                    "--since",
                    SINCE,
                    *extra,
                ]
            )
        self.assertEqual(rc, 0)
        return json.loads(out.getvalue())

    def digest_text(self) -> str:
        return (self.base / sd.DIGEST_DIR / "digest-2026-10-24.md").read_text()

    def twin(self) -> dict:
        return json.loads(
            (self.base / sd.DIGEST_DIR / "digest-2026-10-24.json").read_text()
        )


class DiscoveryTests(DigestFixture):
    def test_only_the_base_and_its_worktrees_are_enumerated(self):
        recent = 1_761_300_000
        mine = self.write(str(self.base), "a", [], mtime=recent)
        tree = self.write(self.worktree, "b", [], mtime=recent)
        # A sibling repo whose slug merely starts with the base's.
        self.write(f"{self.base}-beta", "c", [], mtime=recent)
        old = self.write(self.worktree, "d", [], mtime=recent - 10_000)
        found = sd.discover(self.projects, self.base, recent - 1)
        self.assertEqual(sorted(found), sorted([mine, tree]))
        self.assertNotIn(old, found)


class SessionAccountTests(unittest.TestCase):
    def test_the_block_runs_to_the_next_heading_verbatim(self):
        text = "done\n\n## Session account\n\n- cost $3\n- 40 turns\n\n## Next\nx"
        self.assertEqual(
            sd.account_block(text),
            ["## Session account", "", "- cost $3", "- 40 turns"],
        )

    def test_a_bold_label_counts_and_absence_is_none(self):
        self.assertEqual(
            sd.account_block("**Session account**: $2"), ["**Session account**: $2"]
        )
        self.assertIsNone(sd.account_block("no account here"))


class EndToEndTests(DigestFixture):
    def build(self):
        cwd = self.worktree
        # A Bedrock task session spanning two UTC days, the first before the
        # window: only its second day is this digest's slice.
        self.write(
            cwd,
            "11111111-aaaa",
            [
                prompt(
                    "2026-10-23T10:00:00.000Z",
                    cwd,
                    "<command-name>/init-pr</command-name>",
                ),
                assistant("m1", "2026-10-23T10:01:00.000Z", cwd, WRITE),
                assistant("m2", "2026-10-24T09:00:00.000Z", cwd, WRITE),
                assistant(
                    "m3",
                    "2026-10-24T09:05:00.000Z",
                    cwd,
                    READ,
                    text="## Session account\n\nShipped the digest.",
                ),
            ],
            mtime=WINDOW_MTIME,
        )
        self.mark("11111111-aaaa", "bedrock")
        # A seat chore in the base checkout, attributed by its prompt.
        base = str(self.base)
        self.write(
            base,
            "22222222-bbbb",
            [
                prompt("2026-10-24T08:00:00.000Z", base, "/chore 2001"),
                assistant("m9", "2026-10-24T08:01:00.000Z", base, READ),
            ],
            mtime=WINDOW_MTIME,
        )
        estimates = self.base / sd.ESTIMATE_DIR
        estimates.mkdir(parents=True)
        (estimates / "11111111-aaaa.json").write_text('{"dollars": 20.0}')

    def test_the_digest_slices_prices_and_attributes_each_session(self):
        self.build()
        summary = self.run_digest()
        self.assertEqual(summary["sessions"], 2)
        rates = sm.RATES_BY_MODEL[MODEL]
        window_bedrock = rates.cache_write_1h + rates.cache_read
        self.assertAlmostEqual(
            summary["dollars"], round(window_bedrock + rates.cache_read, 2)
        )
        text = self.digest_text()
        # Calibration leads the header, before any figure.
        self.assertLess(text.index("**Rate calibration**"), text.index("**Window**"))
        self.assertIn("### ENG-2170 · task · bedrock · `11111111`", text)
        self.assertIn("### ENG-2001 · chore · seat · `22222222`", text)
        self.assertIn(
            f"- 2026-10-24: {sm.money(window_bedrock)}, 2 turn(s); cache re-warm "
            f"{sm.money(rates.cache_write_1h)}",
            text,
        )
        self.assertNotIn("- 2026-10-23:", text)
        self.assertIn(f"{sm.money(rates.cache_read)} ({sm.NOT_BILLED})", text)
        cumulative = 2 * rates.cache_write_1h + rates.cache_read
        self.assertIn(f"**Cumulative**: {sm.money(cumulative)}, 3 turn(s)", text)
        self.assertIn(f"**Exit estimate**: $20 vs {sm.money(cumulative)} here", text)
        self.assertIn("> ## Session account", text)
        self.assertIn("> Shipped the digest.", text)
        self.assertIn("**Session account**: no account", text)
        twin = self.twin()
        self.assertEqual(
            {s["issue"] for s in twin["sessions"]}, {"ENG-2170", "ENG-2001"}
        )
        self.assertAlmostEqual(twin["figures"]["rewarm"], rates.cache_write_1h)

    def test_a_ballooned_figure_is_flagged_against_its_median(self):
        self.build()
        history = self.base / sd.DIGEST_DIR
        history.mkdir(parents=True, exist_ok=True)
        lines = [
            {"date": f"2026-10-{d}", "bedrock": 1.0, "seat": 10.0, "models": {}}
            for d in (18, 19, 20)
        ]
        (history / sd.HISTORY_NAME).write_text(
            "".join(json.dumps(e) + "\n" for e in lines)
        )
        summary = self.run_digest()
        text = self.digest_text()
        self.assertIn("(3-digest median $1.00) FLAGGED", text)
        self.assertNotIn("median $10) FLAGGED", text)
        self.assertGreaterEqual(summary["flagged"], 1)
        # This run joined the history, once, even when rerun the same day.
        self.run_digest()
        dates = [
            json.loads(ln)["date"]
            for ln in (history / sd.HISTORY_NAME).read_text().splitlines()
        ]
        self.assertEqual(dates.count("2026-10-24"), 1)

    def test_a_session_with_nothing_in_the_window_is_skipped(self):
        cwd = self.worktree
        self.write(
            cwd,
            "33333333-cccc",
            [assistant("m1", "2026-10-22T10:00:00.000Z", cwd, READ)],
            mtime=WINDOW_MTIME,
        )
        self.assertEqual(self.run_digest()["sessions"], 0)

    def test_a_locked_worktree_is_live(self):
        self.build()
        lock = self.base / ".git" / "worktrees" / "2170"
        lock.mkdir(parents=True)
        (lock / "locked").write_text("")
        self.run_digest()
        self.assertIn("`11111111` · live", self.digest_text())


class CalibrationTests(unittest.TestCase):
    def calibration(self, matches, priced, billed):
        return {
            "day": "2026-10-22",
            "tolerance": 0.15,
            "reconcile": {
                "metered": 90.0,
                "expected": 98.0,
                "billed_matched": 100.0,
                "drift": -0.02,
                "flagged": False,
            },
            "rates": {
                "services": {
                    "Claude Opus 5.5 (Amazon Bedrock Edition)": {
                        "check": {
                            "model": MODEL,
                            "priced": priced,
                            "billed": billed,
                            "matches": matches,
                        }
                    }
                }
            },
        }

    def test_each_rate_row_carries_its_verified_date(self):
        flags: list[str] = []
        lines = sd.calibration_lines(self.calibration(True, 100.0, 100.0), flags)
        self.assertIn("drift -2.0% (tolerance 15%)", lines[0])
        row = next(ln for ln in lines if MODEL in ln)
        self.assertIn(f"verified {sm.RATES_BY_MODEL[MODEL].verified}", row)
        self.assertNotIn("FLAGGED", row)
        idle_row = next(ln for ln in lines if "claude-haiku-5-5" in ln)
        self.assertIn("unverified): not billed on 2026-10-22", idle_row)
        self.assertEqual(flags, [])

    def test_a_row_past_tolerance_is_flagged(self):
        flags: list[str] = []
        lines = sd.calibration_lines(self.calibration(False, 110.0, 100.0), flags)
        row = next(ln for ln in lines if MODEL in ln)
        self.assertIn("drift +10.0% FLAGGED", row)
        self.assertEqual(len(flags), 1)

    def test_no_reading_says_why(self):
        lines = sd.calibration_lines({"error": "the fleet-cost read failed"}, [])
        self.assertEqual(
            lines, ["**Rate calibration**: no reading — the fleet-cost read failed."]
        )


if __name__ == "__main__":
    unittest.main()
