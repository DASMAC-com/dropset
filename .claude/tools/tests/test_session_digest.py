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
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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


class LastCloseTests(unittest.TestCase):
    def test_the_newest_parseable_until_is_the_close(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self.assertIsNone(sd.last_close(directory))
            lines = [
                {"date": "2026-10-23", "until": "2026-10-23T11:55:00"},
                {"date": "2026-10-24", "until": "2026-10-24T11:55:00"},
                {"date": "2026-10-22", "until": "garbage"},
                {"date": "2026-10-21"},
            ]
            (directory / sd.HISTORY_NAME).write_text(
                "".join(json.dumps(e) + "\n" for e in lines) + "not json\n"
            )
            self.assertEqual(
                sd.last_close(directory).isoformat(), "2026-10-24T11:55:00+00:00"
            )


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
        # The sidecar sits beside the substrate record `mark` wrote.
        estimate = self.base / sd.ESTIMATE_DIR / f"11111111-aaaa{sd.ESTIMATE_SUFFIX}"
        estimate.write_text('{"session_id": "11111111-aaaa", "cost_usd": 20.0}')

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

    def test_a_transcript_written_in_the_last_hour_is_live(self):
        self.build()
        recent = 1_792_843_200 - 600  # ten minutes before NOW
        for path in self.projects.rglob("11111111-aaaa.jsonl"):
            os.utime(path, (recent, recent))
        self.run_digest()
        self.assertIn("`11111111` · live", self.digest_text())
        self.assertNotIn("`22222222` · live", self.digest_text())

    def test_sub_agents_render_one_line_per_model(self):
        self.build()
        cwd = self.worktree
        sub = self.projects / sm.slugify(Path(cwd)) / "11111111-aaaa" / "subagents"
        sub.mkdir(parents=True)
        lane = assistant("s1", "2026-10-24T09:01:00.000Z", cwd, READ)
        for name in ("a", "b"):
            (sub / f"{name}.jsonl").write_text(
                json.dumps({**lane, "message": {**lane["message"], "id": f"s-{name}"}})
                + "\n"
            )
        summary = self.run_digest()
        rates = sm.RATES_BY_MODEL[MODEL]
        text = self.digest_text()
        self.assertIn(
            f"  - sub-agents `{MODEL}` ×2 {sm.money(2 * rates.cache_read)}", text
        )
        self.assertIn(f"  - main `{MODEL}`", text)
        twin = self.twin()
        expected = 2 * rates.cache_read / twin["figures"]["total"]
        self.assertAlmostEqual(twin["figures"]["subagent_share"], expected)
        self.assertEqual(summary["sessions"], 2)

    def test_an_unpriced_model_leaves_the_priced_dollars_in_the_totals(self):
        cwd = self.worktree
        unpriced = assistant("u1", "2026-10-24T09:00:00.000Z", cwd, READ)
        unpriced["message"]["model"] = "claude-unknown-9"
        self.write(
            cwd,
            "44444444-dddd",
            [assistant("p1", "2026-10-24T08:00:00.000Z", cwd, READ), unpriced],
            mtime=WINDOW_MTIME,
        )
        self.mark("44444444-dddd", "bedrock")
        summary = self.run_digest()
        read = sm.RATES_BY_MODEL[MODEL].cache_read
        self.assertAlmostEqual(summary["dollars"], round(read, 2))
        twin = self.twin()
        self.assertAlmostEqual(twin["figures"]["bedrock"], read)
        self.assertAlmostEqual(twin["figures"]["models"][MODEL], read)
        self.assertIn("**Unpriced**: `claude-unknown-9`", self.digest_text())

    def test_each_window_opens_where_the_last_one_closed(self):
        cwd = self.worktree
        records = [
            assistant("w1", "2026-10-24T10:00:00.000Z", cwd, READ),
            assistant("w2", "2026-10-25T10:00:00.000Z", cwd, READ),
        ]
        # Written after the first close, so the next pass rediscovers it.
        self.write(cwd, "55555555-eeee", records, mtime=1_792_886_400)
        self.mark("55555555-eeee", "bedrock")
        read = sm.RATES_BY_MODEL[MODEL].cache_read
        # The first digest closes a margin before NOW (10-24 11:55): it holds
        # w1 only, even though w2 is already on disk.
        self.assertAlmostEqual(self.run_digest()["dollars"], round(read, 2))
        # The next pass hands a stale --since, as after a failed stamp; the
        # window still opens at the last close, so w1 is not counted again.
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            sm.main(
                [
                    "digest",
                    "--base",
                    str(self.base),
                    "--projects",
                    str(self.projects),
                    "--now",
                    "2026-10-25T12:00:00Z",
                    "--since",
                    "2026-10-22T00:00:00Z",
                ]
            )
        self.assertAlmostEqual(json.loads(out.getvalue())["dollars"], round(read, 2))
        history = [
            json.loads(ln)
            for ln in (self.base / sd.DIGEST_DIR / sd.HISTORY_NAME)
            .read_text()
            .splitlines()
        ]
        self.assertEqual(
            [(e["since"], e["until"]) for e in history],
            [
                ("2026-10-23T12:00:00", "2026-10-24T11:55:00"),
                ("2026-10-24T11:55:00", "2026-10-25T11:55:00"),
            ],
        )

    def test_a_malformed_history_bound_is_skipped_not_the_history(self):
        directory = self.base / sd.DIGEST_DIR
        directory.mkdir(parents=True)
        entries = [
            {"date": "2026-10-22", "until": "2026-10-23T11:55:00"},
            {"date": "2026-10-23", "until": 1234},
        ]
        (directory / sd.HISTORY_NAME).write_text(
            "".join(json.dumps(e) + "\n" for e in entries)
        )
        given = sd._instant("2026-10-20T00:00:00Z")
        start = sd.window_start(directory, "2026-10-24", given)
        self.assertEqual(start, sd._instant("2026-10-23T11:55:00Z"))

    def test_a_same_day_rerun_reopens_that_days_window(self):
        directory = self.base / sd.DIGEST_DIR
        directory.mkdir(parents=True)
        entries = [
            {
                "date": "2026-10-23",
                "since": "2026-10-22T09:00:00",
                "until": "2026-10-23T09:00:00",
            },
            {
                "date": "2026-10-24",
                "since": "2026-10-23T09:00:00",
                "until": "2026-10-24T09:00:00",
            },
        ]
        (directory / sd.HISTORY_NAME).write_text(
            "".join(json.dumps(e) + "\n" for e in entries)
        )
        start = sd.window_start(directory, "2026-10-24", sd._instant(NOW))
        self.assertEqual(start, sd._instant("2026-10-23T09:00:00Z"))

    def test_a_locked_worktree_is_live(self):
        self.build()
        lock = self.base / ".git" / "worktrees" / "2170"
        lock.mkdir(parents=True)
        (lock / "locked").write_text("")
        self.run_digest()
        self.assertIn("`11111111` · live", self.digest_text())


def queue_run(rid: int, pr: int, created: str, conclusion: str) -> dict:
    return {
        "id": rid,
        "head_branch": f"{sd.ci_metrics.QUEUE_PREFIX}{pr}-abc",
        "created_at": created,
        "conclusion": conclusion,
        "status": "completed",
        "run_attempt": 1,
    }


# One failed queue entry in the window, no baseline.
CI_PAYLOAD = {
    "since": "2026-10-23T12:00:00Z",
    "until": "2026-10-24T11:55:00Z",
    "runs": [queue_run(1, 700, "2026-10-24T01:00:00Z", "failure")],
    "jobs": {
        "1": [
            {
                "name": "Lint",
                "started_at": "2026-10-24T01:00:10Z",
                "completed_at": "2026-10-24T01:08:10Z",
                "conclusion": "failure",
            }
        ]
    },
    "prs": {},
}


class WaitSectionTests(DigestFixture):
    def test_the_wait_section_sits_between_ci_and_sessions(self):
        state = self.base / sd.sm.SUBSTRATE_DIR
        (state / "sessions").mkdir(parents=True, exist_ok=True)
        (state / sd.machine_load.SAMPLE_LOG).write_text(
            "2026-10-24T03:10:00Z builders=4 builds=1 waiting=test:2"
            " longest_s=300 in_use_gib=33.0\n"
        )
        (state / "sessions" / "11111111.load").write_text(
            json.dumps(
                {
                    "at": "2026-10-24T03:09:00Z",
                    "class": "building",
                    "longest_wait_s": 240,
                    "building": 4,
                    "free": 2,
                    "in_use": 30 * 1024**3,
                    "verdict": {"slots": 0, "reasons": ["memory"]},
                }
            )
            + "\n"
        )
        self.run_digest()
        text = self.digest_text()
        self.assertLess(text.index("## CI"), text.index("## Wait"))
        self.assertLess(text.index("## Wait"), text.index("## Sessions"))
        wait = text.split("## Wait")[1].split("## Sessions")[0]
        self.assertIn("Queue profile by hour (depth / longest wait): 03h 2/300s", wait)
        self.assertIn("memory refusals 1; peak in_use 33.0 GiB", wait)
        self.assertIn("**For the analyst**", wait)
        self.assertEqual(self.twin()["wait"]["launches"], 1)


class CiSectionTests(DigestFixture):
    def test_without_ci_the_section_says_not_requested(self):
        self.run_digest()
        self.assertIn("## CI\n\nCI section not requested", self.digest_text())
        self.assertIsNone(self.twin()["ci"])

    def test_a_failed_entry_is_a_flagged_header_line_and_in_the_twin(self):
        with mock.patch.object(
            sd.ci_metrics, "fetch_day", return_value=CI_PAYLOAD
        ) as fetch:
            summary = self.run_digest("--ci")
        since, until, cache, budget = fetch.call_args.args
        self.assertEqual(since.isoformat(), "2026-10-23T12:00:00+00:00")
        self.assertEqual(until.isoformat(), "2026-10-24T11:55:00+00:00")
        self.assertEqual(cache, self.base / sd.DIGEST_DIR / sd.CI_CACHE_NAME)
        self.assertEqual(budget, sd.CI_BUDGET_SECONDS)
        self.assertEqual(summary["flagged"], 1)
        text = self.digest_text()
        self.assertIn("  - CI pr-700 entry not all-green ['Lint=failure']", text)
        self.assertIn("NOT GREEN", text.split("## CI")[1])
        self.assertEqual(self.twin()["ci"]["entries"][0]["pr"], "700")

    def test_unreadable_github_still_writes_the_digest(self):
        err = subprocess.CalledProcessError(
            1, ["gh"], stderr="HTTP 403: API rate limit exceeded\nmore"
        )
        with mock.patch.object(sd.ci_metrics, "fetch_day", side_effect=err):
            summary = self.run_digest("--ci")
        # Flagged, so a CI read that stays broken never reads as healthy.
        self.assertEqual(summary["flagged"], 1)
        line = "CI data unavailable: gh exited 1: HTTP 403: API rate limit exceeded"
        self.assertIn(f"  - {line}", self.digest_text())
        self.assertIn("error", self.twin()["ci"])

    def test_any_other_failure_still_writes_the_digest(self):
        cases = [
            (subprocess.TimeoutExpired(["gh"], 60), "a gh read timed out after 60s"),
            (FileNotFoundError("gh"), "FileNotFoundError: gh"),
            (TimeoutError("job reads passed"), "TimeoutError: job reads passed"),
            # `repository: null` in a GraphQL reply.
            (AttributeError("'NoneType' object"), "AttributeError: 'NoneType'"),
        ]
        for exc, expected in cases:
            with self.subTest(exc=type(exc).__name__):
                with mock.patch.object(sd.ci_metrics, "fetch_day", side_effect=exc):
                    self.run_digest("--ci")
                self.assertIn(f"CI data unavailable: {expected}", self.digest_text())


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

    def test_a_mismatch_billed_at_zero_is_still_flagged(self):
        flags: list[str] = []
        lines = sd.calibration_lines(self.calibration(False, 5.0, 0.0), flags)
        row = next(ln for ln in lines if MODEL in ln)
        self.assertIn("drift n/a FLAGGED", row)
        self.assertEqual(len(flags), 1)

    def test_a_billed_service_with_no_rate_row_is_flagged(self):
        calibration = self.calibration(True, 100.0, 100.0)
        calibration["rates"]["services"]["Claude Opus 9.1 (Amazon Bedrock Edition)"] = {
            "check": {"model": None, "priced": None, "billed": None, "matches": None}
        }
        flags: list[str] = []
        lines = sd.calibration_lines(calibration, flags)
        self.assertIn(
            "- Claude Opus 9.1: billed on 2026-10-22 with no rate row — FLAGGED", lines
        )
        self.assertEqual(len(flags), 1)

    def test_a_missing_reconcile_figure_is_no_reading_not_zero(self):
        calibration = self.calibration(True, 100.0, 100.0)
        calibration["reconcile"] = {}
        lines = sd.calibration_lines(calibration, [])
        self.assertEqual(
            lines,
            [
                "**Rate calibration**: no reading — 2026-10-22 carried no reconcile figure."
            ],
        )


class HeadScanTests(DigestFixture):
    def head(self, records):
        return sd.scan_head(self.write(self.worktree, "h", records))

    def test_local_commands_neither_name_the_verb_nor_spend_the_budget(self):
        cwd, t = self.worktree, "2026-10-24T08:00:00.000Z"
        head = self.head(
            [
                prompt(
                    t, cwd, "<local-command-caveat>Caveat: …</local-command-caveat>"
                ),
                prompt(t, cwd, "<command-name>/model</command-name>"),
                prompt(
                    t, cwd, "<local-command-stdout>Set model</local-command-stdout>"
                ),
                prompt(t, cwd, "<command-name>/clear</command-name>"),
                prompt(t, cwd, "<command-name>/chore</command-name><command-args>2001"),
            ]
        )
        self.assertEqual(head.verb, "chore")
        self.assertEqual(head.issue, "ENG-2001")

    def test_a_prompt_opening_with_a_path_names_no_verb(self):
        t = "2026-10-24T08:00:00.000Z"
        head = self.head([prompt(t, self.worktree, "/Users/x/repo is broken")])
        self.assertIsNone(head.verb)


if __name__ == "__main__":
    unittest.main()
