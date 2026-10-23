"""Tests for ``fleet_cost.py``: the deduplicated transcript walk, substrate
classification, issue attribution, and the two bill comparisons.

The fixture is a throwaway Claude home and base repo: a Bedrock worktree
session whose one message is streamed as three lines repeating the same usage
(the 2.2-2.6x inflation the dedup exists to prevent), a resumed transcript that
carries that message forward again, a sub-agent, a seat base-repo session, and
an unmarked worktree session.
"""

from __future__ import annotations

import contextlib
import datetime
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import fleet_cost as fc
import session_metrics as sm

DAY = "2026-10-13"
USAGE = {
    "input_tokens": 1_000_000,
    "output_tokens": 1_000_000,
    "cache_creation_input_tokens": 0,
    "cache_read_input_tokens": 0,
}
# One million input plus one million output at the Opus 5.5 rate.
ONE_MESSAGE = (
    sm.RATES_BY_MODEL["claude-opus-5-5"].input
    + sm.RATES_BY_MODEL["claude-opus-5-5"].output
)
# A Claude-shaped id with no rate row, so it bills under a named service but
# refuses to price.
UNPRICED_MODEL = "claude-opus-9-1"


def record(
    msg_id: str, cwd: str, day: str = DAY, model: str = "claude-opus-5-5"
) -> str:
    return json.dumps(
        {
            "cwd": cwd,
            "timestamp": f"{day}T12:00:00.000Z",
            "message": {"id": msg_id, "model": model, "usage": USAGE},
        }
    )


class WalkTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.home = root / "home"
        self.projects = self.home / "projects"
        base = root / "repo"
        markers = base / sm.SUBSTRATE_DIR
        markers.mkdir(parents=True)
        (markers / "1651").write_text("bedrock\n")
        (markers / "eng-1073").write_text("bedrock\n")
        (markers / "1700").write_text("anthropic\n")
        (markers / "1701").write_text("garbage\n")
        worker = f"{base}/.claude/worktrees/1651"
        legacy = f"{base}/.claude/worktrees/eng-1073"
        unmarked = f"{base}/.claude/worktrees/1999"

        bedrock = self.projects / "bedrock"
        (bedrock / "s1" / "subagents").mkdir(parents=True)
        # One message streamed as three content-block lines, then one on a
        # model with no rate (a fictional one, so a new rate row cannot price
        # it).
        self.write(
            bedrock / "s1.jsonl",
            [record("msg_a", worker)] * 3
            + [record("msg_unpriced", worker, model=UNPRICED_MODEL)],
        )
        self.write(
            bedrock / "s1" / "subagents" / "agent.jsonl", [record("msg_sub", worker)]
        )
        # A resumed session repeating msg_a, plus one new message and one
        # outside the window.
        self.write(
            bedrock / "s2.jsonl",
            [
                record("msg_a", legacy),
                record("msg_b", legacy),
                record("msg_old", legacy, day="2026-10-12"),
                "not json",
            ],
        )
        self.write(self.projects / "seat" / "s3.jsonl", [record("msg_c", str(base))])
        self.write(self.projects / "loose" / "s4.jsonl", [record("msg_d", unmarked)])
        seat_worktree = f"{base}/.claude/worktrees/1700"
        garbled = f"{base}/.claude/worktrees/1701"
        self.write(
            self.projects / "seat" / "s5.jsonl", [record("msg_e", seat_worktree)]
        )
        self.write(self.projects / "loose" / "s6.jsonl", [record("msg_f", garbled)])
        self.meter = fc.walk(self.projects, [DAY])

    def tearDown(self) -> None:
        self.tmp.cleanup()

    @staticmethod
    def write(path: Path, lines: list[str]) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(lines) + "\n")

    def test_streamed_and_resumed_lines_count_once(self) -> None:
        summary = fc.day_summary(self.meter, DAY)
        # msg_a once, msg_sub, msg_b: three messages, not five.
        self.assertAlmostEqual(
            summary["substrates"]["bedrock"]["dollars"], 3 * ONE_MESSAGE
        )
        self.assertEqual(summary["substrates"]["bedrock"]["sessions"], 2)
        self.assertEqual(self.meter.parse_errors, 1)

    def test_substrates_are_split_and_unmarked_is_not_priced_in(self) -> None:
        summary = fc.day_summary(self.meter, DAY)
        # The base-repo session and the `anthropic`-marked worktree.
        self.assertEqual(summary["substrates"]["seat"]["sessions"], 2)
        # The unmarked worktree and the one whose marker is unrecognized.
        self.assertEqual(summary["substrates"]["unattributed"]["sessions"], 2)
        services = summary["bedrock_by_service"]
        self.assertEqual(list(services), ["Claude Opus 5.5 (Amazon Bedrock Edition)"])
        self.assertAlmostEqual(sum(services.values()), 3 * ONE_MESSAGE)

    def test_an_unpriced_model_is_named_and_kept_out_of_the_services(self) -> None:
        summary = fc.day_summary(self.meter, DAY)
        self.assertEqual(summary["unpriced"], [UNPRICED_MODEL])
        self.assertIn(UNPRICED_MODEL, summary["bedrock_tokens"])
        rec = fc.reconcile_day(
            summary,
            {
                "Claude Opus 5.5 (Amazon Bedrock Edition)": 3 * ONE_MESSAGE / 0.918,
                "Claude Opus 9.1 (Amazon Bedrock Edition)": 4.0,
            },
            0.15,
        )
        self.assertAlmostEqual(rec["billed_unpriced"], 4.0)
        self.assertEqual(rec["billed_background"], 0.0)
        self.assertFalse(rec["flagged"])

    def test_a_transcript_unreadable_mid_walk_is_counted_not_fatal(self) -> None:
        # As if a purge removed every file between the glob and the open.
        with mock.patch.object(fc.sm, "_read_lines", side_effect=OSError("gone")):
            meter = fc.walk(self.projects, [DAY])
        self.assertEqual(meter.unreadable, 6)
        self.assertEqual(meter.by_day, {})

    def test_main_meters_a_past_day_and_refuses_today(self) -> None:
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(fc.sm, "claude_home", return_value=self.home):
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                self.assertEqual(fc.main(["meter", "--day", DAY]), 0)
                self.assertEqual(fc.main(["meter", "--day", "2999-01-01"]), 2)
        self.assertIn(f"{DAY} (UTC)", out.getvalue())
        self.assertIn("costliest issues: ENG-1651", out.getvalue())
        self.assertIn("not a complete UTC day", err.getvalue())

    def test_issue_attribution_reads_both_tag_spellings(self) -> None:
        costs = dict(fc.issue_costs(self.meter, [DAY]))
        self.assertAlmostEqual(costs["ENG-1651"], 2 * ONE_MESSAGE)
        self.assertAlmostEqual(costs["ENG-1073"], ONE_MESSAGE)

    def test_every_substrate_carries_its_issue(self) -> None:
        # The seat worktree (1700) and the unmarked one (1999) are keyed to
        # their issues the same way the Bedrock ones are, and listed, not priced.
        self.assertEqual(
            fc.issue_session_counts(self.meter, [DAY], fc.SEAT),
            [("(no issue)", 1), ("ENG-1700", 1)],
        )
        self.assertEqual(
            fc.issue_session_counts(self.meter, [DAY], fc.UNATTRIBUTED),
            [("ENG-1701", 1), ("ENG-1999", 1)],
        )

    def test_a_base_checkout_chore_is_keyed_by_its_bootstrap_prompt(self) -> None:
        base = str(Path(self.tmp.name) / "repo")
        prompt = json.dumps(
            {
                "type": "user",
                "cwd": base,
                "timestamp": f"{DAY}T11:59:00.000Z",
                "message": {
                    "role": "user",
                    "content": "<command-message>chore</command-message>\n"
                    "<command-name>/chore</command-name>\n"
                    "<command-args>2031</command-args>",
                },
            }
        )
        # A caveat record ahead of the command must not hide it.
        caveat = json.dumps(
            {
                "type": "user",
                "isMeta": True,
                "cwd": base,
                "message": {"role": "user", "content": "<local-command-caveat>"},
            }
        )
        self.write(
            self.projects / "seat" / "s7.jsonl",
            [caveat, prompt, record("msg_g", base)],
        )
        meter = fc.walk(self.projects, [DAY])
        self.assertIn(("ENG-2031", 1), fc.issue_session_counts(meter, [DAY], fc.SEAT))

    def test_issue_of_reads_every_pre_rename_spelling(self) -> None:
        for tag in ("1651", "eng-1651", "ceo-1651", "exp-1651"):
            cwd = f"/repo/.claude/worktrees/{tag}"
            self.assertEqual(fc.issue_of(cwd), "ENG-1651", tag)
        self.assertEqual(fc.issue_of_prompt("/architect ENG-1572"), "ENG-1572")
        self.assertIsNone(fc.issue_of_prompt("look at the plan"))

    def test_stale_transcript_is_skipped_unread(self) -> None:
        stale = self.projects / "bedrock" / "s2.jsonl"
        os.utime(stale, (0, 0))
        summary = fc.day_summary(fc.walk(self.projects, [DAY]), DAY)
        self.assertAlmostEqual(
            summary["substrates"]["bedrock"]["dollars"], 2 * ONE_MESSAGE
        )


class UnitTest(unittest.TestCase):
    def test_service_for_model(self) -> None:
        cases = {
            "claude-opus-5-5": "Claude Opus 5.5 (Amazon Bedrock Edition)",
            "claude-opus-5": "Claude Opus 5 (Amazon Bedrock Edition)",
            "claude-fable-5-1": "Claude Fable 5.1 (Amazon Bedrock Edition)",
            "claude-sonnet-5-5": "Claude Sonnet 5.5 (Amazon Bedrock Edition)",
            "claude-haiku-4-5-20251001": "Claude Haiku 4.5 (Amazon Bedrock Edition)",
            "us.anthropic.claude-haiku-4-5-20251001-v1:0": (
                "Claude Haiku 4.5 (Amazon Bedrock Edition)"
            ),
            "global.anthropic.claude-opus-5-5-v1:0": (
                "Claude Opus 5.5 (Amazon Bedrock Edition)"
            ),
        }
        for model, service in cases.items():
            self.assertEqual(fc.service_for_model(model), service, model)
        self.assertIsNone(fc.service_for_model("<synthetic>"))

    def test_parse_billed_keeps_only_edition_services(self) -> None:
        response = {
            "ResultsByTime": [
                {
                    "TimePeriod": {"Start": DAY},
                    "Groups": [
                        {
                            "Keys": ["Claude Opus 5.5 (Amazon Bedrock Edition)"],
                            "Metrics": {"UnblendedCost": {"Amount": "190.83"}},
                        },
                        {
                            "Keys": ["Amazon Bedrock"],
                            "Metrics": {"UnblendedCost": {"Amount": "0"}},
                        },
                    ],
                }
            ]
        }
        self.assertEqual(
            fc.parse_billed(response),
            {DAY: {"Claude Opus 5.5 (Amazon Bedrock Edition)": 190.83}},
        )

    def test_estimate_normalizes_the_local_offset(self) -> None:
        # The 10-13 UTC bucket, as the CLI renders it on a Pacific machine.
        response = {
            "MetricDataResults": [
                {
                    "Id": "m1",
                    "Timestamps": ["2026-10-12T17:00:00-07:00"],
                    "Values": [2e6],
                }
            ]
        }
        params = {"TokenRateOutput": 22.0}
        estimate = fc.estimate_from(response, params, ["2026-10-12", DAY])
        self.assertEqual(estimate, {"2026-10-12": 0.0, DAY: 44.0})

    def test_reconcile_flags_after_the_classifier_overhead(self) -> None:
        service = "Claude Opus 5.5 (Amazon Bedrock Edition)"
        summary = {"bedrock_by_service": {service: 91.8}, "unpriced": []}
        on_baseline = fc.reconcile_day(summary, {service: 100.0, "Other": 1.0}, 0.15)
        self.assertFalse(on_baseline["flagged"])
        self.assertAlmostEqual(on_baseline["drift"], 0.0)
        self.assertAlmostEqual(on_baseline["billed_background"], 1.0)
        leaking = fc.reconcile_day(summary, {service: 130.0}, 0.15)
        self.assertTrue(leaking["flagged"])
        no_bill = fc.reconcile_day(summary, {}, 0.15)
        self.assertTrue(no_bill["flagged"])

    def test_reconcile_flags_a_large_background_share(self) -> None:
        # Every session misclassified: the session model's own service lands as
        # background, which must not read as a clean day.
        summary = {"bedrock_by_service": {}, "unpriced": []}
        rec = fc.reconcile_day(
            summary, {"Claude Opus 5.5 (Amazon Bedrock Edition)": 190.0}, 0.15
        )
        self.assertAlmostEqual(rec["billed_background"], 190.0)
        self.assertTrue(rec["flagged"])

    def test_reconcile_does_not_judge_a_quiet_day(self) -> None:
        # Cents of harness calls and no sessions: all background, but too small
        # to mean anything.
        summary = {"bedrock_by_service": {}, "unpriced": []}
        rec = fc.reconcile_day(
            summary, {"Claude Haiku 5.5 (Amazon Bedrock Edition)": 0.5}, 0.15
        )
        self.assertFalse(rec["flagged"])

    def test_spend_cap_verdicts(self) -> None:
        cases = [
            ((192.44, 190.88), "ok", False),
            ((100.0, 130.0), "cap LEAKING", True),
            ((130.0, 100.0), "trips EARLY", True),
            # A real estimate against no bill trips early, as reconcile flags it.
            ((50.0, 0.0), "trips EARLY", True),
            # Below the judging floor, a day is reported but never flagged.
            ((2.0, 0.0), "ok", False),
            ((0.5, 3.0), "ok", False),
        ]
        for (estimate, gross), verdict, flagged in cases:
            cap = fc.spend_cap_day(estimate, gross, 0.15)
            self.assertEqual(cap["verdict"], verdict, (estimate, gross))
            self.assertEqual(cap["flagged"], flagged, (estimate, gross))

    def test_resolve_days_defaults_canonicalizes_and_refuses_today(self) -> None:
        today = datetime.date(2026, 10, 15)
        self.assertEqual(fc.resolve_days(None, today), ["2026-10-13", "2026-10-14"])
        self.assertEqual(
            fc.resolve_days(["2026-10-13", "2026-10-12", "2026-10-13"], today),
            ["2026-10-12", "2026-10-13"],
        )
        with self.assertRaises(ValueError):
            fc.resolve_days(["2026-10-15"], today)


if __name__ == "__main__":
    unittest.main()
