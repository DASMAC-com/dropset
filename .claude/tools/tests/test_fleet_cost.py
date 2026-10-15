"""Tests for ``fleet_cost.py``: the deduplicated transcript walk, substrate
classification, issue attribution, and the two bill comparisons.

The fixture is a throwaway Claude home and base repo: a Bedrock worktree
session whose one message is streamed as three lines repeating the same usage
(the 2.2-2.6x inflation the dedup exists to prevent), a resumed transcript that
carries that message forward again, a sub-agent, a seat base-repo session, and
an unmarked worktree session.
"""

from __future__ import annotations

import datetime
import json
import os
import tempfile
import unittest
from pathlib import Path

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
        self.projects = root / "home" / "projects"
        base = root / "repo"
        markers = base / sm.SUBSTRATE_DIR
        markers.mkdir(parents=True)
        (markers / "1651").write_text("bedrock\n")
        (markers / "eng-1073").write_text("bedrock\n")
        worker = f"{base}/.claude/worktrees/1651"
        legacy = f"{base}/.claude/worktrees/eng-1073"
        unmarked = f"{base}/.claude/worktrees/1999"

        bedrock = self.projects / "bedrock"
        (bedrock / "s1" / "subagents").mkdir(parents=True)
        # One message streamed as three content-block lines.
        self.write(bedrock / "s1.jsonl", [record("msg_a", worker)] * 3)
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
        self.assertEqual(summary["substrates"]["seat"]["sessions"], 1)
        self.assertEqual(summary["substrates"]["unattributed"]["sessions"], 1)
        services = summary["bedrock_by_service"]
        self.assertEqual(list(services), ["Claude Opus 5.5 (Amazon Bedrock Edition)"])
        self.assertAlmostEqual(sum(services.values()), 3 * ONE_MESSAGE)

    def test_issue_attribution_reads_both_tag_spellings(self) -> None:
        costs = dict(fc.issue_costs(self.meter, [DAY]))
        self.assertAlmostEqual(costs["ENG-1651"], 2 * ONE_MESSAGE)
        self.assertAlmostEqual(costs["ENG-1073"], ONE_MESSAGE)

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
            "claude-haiku-4-5-20251001": "Claude Haiku 4.5 (Amazon Bedrock Edition)",
            "us.anthropic.claude-haiku-4-5-20251001-v1:0": (
                "Claude Haiku 4.5 (Amazon Bedrock Edition)"
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
        summary = {"bedrock_by_service": {service: 91.8}}
        on_baseline = fc.reconcile_day(summary, {service: 100.0, "Other": 1.0}, 0.15)
        self.assertFalse(on_baseline["flagged"])
        self.assertAlmostEqual(on_baseline["drift"], 0.0)
        self.assertAlmostEqual(on_baseline["billed_background"], 1.0)
        leaking = fc.reconcile_day(summary, {service: 130.0}, 0.15)
        self.assertTrue(leaking["flagged"])
        no_bill = fc.reconcile_day(summary, {}, 0.15)
        self.assertTrue(no_bill["flagged"])

    def test_resolve_days_defaults_and_refuses_today(self) -> None:
        today = datetime.date(2026, 10, 15)
        self.assertEqual(fc.resolve_days(None, today), ["2026-10-13", "2026-10-14"])
        with self.assertRaises(ValueError):
            fc.resolve_days(["2026-10-15"], today)


if __name__ == "__main__":
    unittest.main()
