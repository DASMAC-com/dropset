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

    def test_seat_spend_is_priced_and_labelled_not_billed(self) -> None:
        summary = fc.day_summary(self.meter, DAY)
        seat = next(ln for ln in fc.render_meter(summary, DAY) if "seat" in ln)
        self.assertIn("at Bedrock rates, not billed", seat)
        self.assertIn("$", seat)
        costs = dict(fc.issue_costs(self.meter, [DAY], fc.SEAT))
        self.assertAlmostEqual(costs["ENG-1700"], ONE_MESSAGE)
        # ...and never into the Bedrock total the bill is compared against.
        self.assertNotIn("ENG-1700", dict(fc.issue_costs(self.meter, [DAY])))

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

    def test_a_session_record_closes_both_blind_spots(self) -> None:
        # A Bedrock `plan` (base repo, no tag) and a worktree session whose
        # tag marker a later anthropic launch rewrote: each session's own
        # record classifies it.
        base = Path(self.tmp.name) / "repo"
        sessions = base / sm.SUBSTRATE_DIR / "sessions"
        sessions.mkdir()
        (sessions / "s8").write_text("bedrock\n")
        (sessions / "s9").write_text("bedrock\n")
        self.write(self.projects / "plan" / "s8.jsonl", [record("msg_h", str(base))])
        relaunched = f"{base}/.claude/worktrees/1700"
        self.write(self.projects / "loose" / "s9.jsonl", [record("msg_i", relaunched)])
        summary = fc.day_summary(fc.walk(self.projects, [DAY]), DAY)
        self.assertEqual(summary["substrates"]["bedrock"]["sessions"], 4)
        # s3 and s5, unchanged: no record, so the old reading stands.
        self.assertEqual(summary["substrates"]["seat"]["sessions"], 2)

    def test_a_bedrock_plan_session_is_priced_under_its_verb(self) -> None:
        base = Path(self.tmp.name) / "repo"
        sessions = base / sm.SUBSTRATE_DIR / "sessions"
        sessions.mkdir()
        (sessions / "s10").write_text("bedrock\n")
        prompt = json.dumps(
            {
                "type": "user",
                "cwd": str(base),
                "message": {
                    "role": "user",
                    "content": "<command-message>plan</command-message>\n"
                    "<command-name>/plan</command-name>",
                },
            }
        )
        self.write(
            self.projects / "plan" / "s10.jsonl", [prompt, record("msg_j", str(base))]
        )
        meter = fc.walk(self.projects, [DAY])
        self.assertAlmostEqual(dict(fc.issue_costs(meter, [DAY]))["plan"], ONE_MESSAGE)
        # And the rendered per-issue line carries it.
        out = io.StringIO()
        with mock.patch.object(fc.sm, "claude_home", return_value=self.home):
            with contextlib.redirect_stdout(out):
                self.assertEqual(fc.main(["meter", "--day", DAY]), 0)
        self.assertRegex(out.getvalue(), r"costliest issues: .*\bplan \$")
        # The seat base-repo session with no command stays `(no issue)`.
        self.assertIn(("(no issue)", 1), fc.issue_session_counts(meter, [DAY], fc.SEAT))

    def test_verb_of_prompt(self) -> None:
        self.assertEqual(fc.verb_of_prompt("/plan"), "plan")
        self.assertEqual(fc.verb_of_prompt("/plan anthropic"), "plan")
        self.assertEqual(fc.verb_of_prompt("/curate"), "curate")
        self.assertEqual(
            fc.verb_of_prompt("<command-name>/curate</command-name>"), "curate"
        )
        self.assertIsNone(fc.verb_of_prompt("/curated list"))
        self.assertEqual(fc.verb_of_prompt("/launch"), "launch")
        self.assertIsNone(fc.verb_of_prompt("/launcher"))
        self.assertEqual(
            fc.verb_of_prompt("<command-name>/housekeeping</command-name>"),
            "housekeeping",
        )
        self.assertIsNone(fc.verb_of_prompt("/planning notes"))
        self.assertIsNone(fc.verb_of_prompt("run /plan later"))
        # An issue-keyed launch is the issue regex's, not this one's.
        self.assertIsNone(fc.verb_of_prompt("/chore 2031"))

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


class FrictionRollupTest(unittest.TestCase):
    """A session that rebased and ran past midnight puts a friction line on
    its day; a clean day prints none."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.home = root / "home"
        base = root / "repo"
        (base / sm.SUBSTRATE_DIR).mkdir(parents=True)
        (base / sm.SUBSTRATE_DIR / "1651").write_text("bedrock\n")
        cwd = f"{base}/.claude/worktrees/1651"

        def line(stamp: str, **fields) -> str:
            return json.dumps({"cwd": cwd, "timestamp": stamp, **fields})

        def turn(msg_id: str, stamp: str, content: list | None = None) -> str:
            message = {"id": msg_id, "model": "claude-opus-5-5", "usage": USAGE}
            return line(stamp, message={**message, "content": content or []})

        rebase = {
            "type": "tool_use",
            "id": "t1",
            "name": "Bash",
            "input": {"command": "git rebase origin/main"},
        }
        result = {"type": "tool_result", "tool_use_id": "t1", "content": "rebased"}
        WalkTest.write(
            self.home / "projects" / "bedrock" / "s1.jsonl",
            [
                turn("m1", "2026-10-12T23:00:00.000Z", [rebase]),
                line("2026-10-12T23:00:05.000Z", message={"content": [result]}),
                turn("m2", "2026-10-12T23:01:00.000Z"),
                turn("m3", "2026-10-13T01:00:00.000Z"),
            ],
        )
        WalkTest.write(
            self.home / "projects" / "bedrock" / "s2.jsonl",
            [turn("m4", "2026-10-14T12:00:00.000Z")],
        )

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_summary_counts_the_sessions_and_prices_after_the_rebase(self) -> None:
        meter = fc.walk(self.home / "projects", ["2026-10-12", "2026-10-14"])
        friction = fc.day_summary(meter, "2026-10-12")["friction"]
        self.assertEqual(friction["spanned_sessions"], 1)
        self.assertEqual(friction["rebased_sessions"], 1)
        # m2 alone: m1 carried the rebase, m3 landed on another day.
        self.assertAlmostEqual(friction["dollars_after"]["bedrock"], ONE_MESSAGE)
        self.assertNotIn("friction", fc.day_summary(meter, "2026-10-14"))

    def test_reconcile_prints_the_rollup_only_on_the_friction_day(self) -> None:
        out = io.StringIO()
        billed = {"2026-10-12": {}, "2026-10-14": {}}
        with (
            mock.patch.object(fc.sm, "claude_home", return_value=self.home),
            mock.patch.object(fc, "fetch_billed", return_value=billed),
            contextlib.redirect_stdout(out),
        ):
            fc.main(["reconcile", "--day", "2026-10-12", "--day", "2026-10-14"])
        text = out.getvalue()
        friction = [ln for ln in text.splitlines() if "friction" in ln]
        self.assertEqual(len(friction), 1)
        self.assertIn(
            "1 session(s) spanned a day, 1 rebased or re-enqueued, 0 stalled",
            friction[0],
        )
        self.assertLess(text.index("friction"), text.index("2026-10-14 (UTC)"))

    def test_a_resumed_child_repeating_the_rebase_is_not_counted_twice(self) -> None:
        parent = self.home / "projects" / "bedrock" / "s1.jsonl"
        child = self.home / "projects" / "bedrock" / "s3.jsonl"
        child.write_text(parent.read_text())
        meter = fc.walk(self.home / "projects", ["2026-10-12"])
        friction = fc.day_summary(meter, "2026-10-12")["friction"]
        self.assertEqual(friction["rebased_sessions"], 1)
        self.assertAlmostEqual(friction["dollars_after"]["bedrock"], ONE_MESSAGE)

    def test_a_child_sorting_before_its_parent_inherits_nothing(self) -> None:
        bedrock = self.home / "projects" / "bedrock"
        parent = bedrock / "s1.jsonl"
        cwd = f"{Path(self.tmp.name)}/repo/.claude/worktrees/1651"
        own = json.dumps(
            {
                "cwd": cwd,
                "timestamp": "2026-10-14T09:00:00.000Z",
                "message": {"id": "m9", "model": "claude-opus-5-5", "usage": USAGE},
            }
        )
        # Named to sort first, but written after the parent it resumed.
        child = bedrock / "s0.jsonl"
        child.write_text(parent.read_text() + own + "\n")
        now = datetime.datetime.now().timestamp()
        os.utime(parent, (now - 100, now - 100))
        os.utime(child, (now, now))
        summary = fc.day_summary(
            fc.walk(self.home / "projects", ["2026-10-14"]), "2026-10-14"
        )
        self.assertNotIn("friction", summary)

    def test_a_stall_alone_puts_the_line_on_its_day(self) -> None:
        cwd = f"{Path(self.tmp.name)}/repo/.claude/worktrees/1651"
        stalled = [
            json.dumps(
                {
                    "cwd": cwd,
                    "timestamp": f"2026-10-14T{hour}:00:00.000Z",
                    "message": {
                        "id": msg_id,
                        "model": "claude-opus-5-5",
                        "usage": USAGE,
                    },
                }
            )
            for msg_id, hour in (("m5", "13"), ("m6", "18"))
        ]
        WalkTest.write(self.home / "projects" / "bedrock" / "s4.jsonl", stalled)
        meter = fc.walk(self.home / "projects", ["2026-10-14"])
        friction = fc.day_summary(meter, "2026-10-14")["friction"]
        self.assertEqual(friction["stalled_sessions"], 1)
        self.assertEqual(friction["spanned_sessions"], 0)
        self.assertAlmostEqual(friction["dollars_after"]["bedrock"], ONE_MESSAGE)


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

    # The 2026-10-13 Opus 5.5 bill by usage type, quantity in millions of
    # tokens as Cost Explorer reports it, each cost rounded to the cent (so the
    # lines sum to $190.84 against the day's billed $190.83).
    OPUS_SERVICE = "Claude Opus 5.5 (Amazon Bedrock Edition)"
    OPUS_USAGE = {
        "USW2-MP:USW2_cache_read_tokens_standard-Units": (387.9575, 85.35),
        "USW2-MP:USW2_cache_write_tokens_1h_standard-Units": (8.0587, 70.92),
        "USW2-MP:USW2_input_tokens_standard-Units": (1.2747, 5.61),
        "USW2-MP:USW2_output_tokens_standard-Units": (1.3163, 28.96),
    }

    def usage_response(
        self, usage: dict[str, tuple[float, float]], service: str = OPUS_SERVICE
    ) -> dict:
        groups = [
            {
                "Keys": [service, usage_type],
                "Metrics": {
                    "UsageQuantity": {"Amount": str(q)},
                    "UnblendedCost": {"Amount": str(d)},
                },
            }
            for usage_type, (q, d) in usage.items()
        ]
        groups.append(
            {
                "Keys": ["Amazon Bedrock", "USW2-ModelUnits"],
                "Metrics": {
                    "UsageQuantity": {"Amount": "1"},
                    "UnblendedCost": {"Amount": "0"},
                },
            }
        )
        return {"ResultsByTime": [{"TimePeriod": {"Start": DAY}, "Groups": groups}]}

    def test_tier_of_maps_only_the_standard_tiers(self) -> None:
        cases = {
            "USW2-MP:USW2_input_tokens_standard-Units": "input",
            "USW2-MP:USW2_output_tokens_standard-Units": "output",
            "USW2-MP:USW2_cache_read_tokens_standard-Units": "cache_read",
            "USW2-MP:USW2_cache_write_tokens_1h_standard-Units": "cache_write_1h",
            "USW2-MP:USW2_InputTokenCount-Units": "input",
            "USW2-MP:USW2_OutputTokenCount-Units": "output",
            "USW2-MP:USW2_cache_write_tokens_5m_standard-Units": None,
            "USW2-MP:USW2_input_tokens_long_context-Units": None,
            "USW2-MP:USW2_input_tokens_priority-Units": None,
            # A qualifier BEFORE the tier must not fold into the standard rate.
            "USW2-MP:USW2_batch_input_tokens_standard-Units": None,
            "USW2-MP:USW2_LongContext_InputTokenCount-Units": None,
        }
        for usage_type, tier in cases.items():
            self.assertEqual(fc.tier_of(usage_type), tier, usage_type)

    def test_rates_reproduce_the_verified_opus_row(self) -> None:
        usage = fc.parse_usage(self.usage_response(self.OPUS_USAGE))
        self.assertEqual(list(usage), [self.OPUS_SERVICE])
        derived = fc.derive_rates(usage[self.OPUS_SERVICE])
        row = sm.RATES_BY_MODEL["claude-opus-5-5"]
        for tier, rate in derived["rates"].items():
            self.assertAlmostEqual(rate, getattr(row, tier), places=2, msg=tier)
        check = fc.check_rates(fc.model_for_service(self.OPUS_SERVICE), derived)
        self.assertEqual(check["model"], "claude-opus-5-5")
        self.assertTrue(check["matches"])
        self.assertAlmostEqual(check["billed"], 190.84, places=2)

    def test_an_unrecognized_tier_is_listed_not_averaged_in(self) -> None:
        usage = {
            **self.OPUS_USAGE,
            "USW2-MP:USW2_input_tokens_long_context-Units": (1.0, 8.80),
        }
        derived = fc.derive_rates(usage)
        self.assertAlmostEqual(derived["rates"]["input"], 4.40, places=2)
        unknown = [t for t in derived["usage_types"] if t["tier"] is None]
        self.assertEqual(len(unknown), 1)
        self.assertAlmostEqual(unknown[0]["rate"], 8.80)
        self.assertIsNone(fc.check_rates("claude-opus-5-5", derived)["matches"])
        status, out = self.run_rates(self.usage_response(usage))
        self.assertEqual(status, 1)
        self.assertIn("UNRECOGNIZED", out)
        self.assertIn("not judged, an unrecognized type billed", out)

    def run_rates(self, response: dict, *flags: str) -> tuple[int, str]:
        out = io.StringIO()
        with (
            mock.patch.object(fc, "aws", return_value=response),
            contextlib.redirect_stdout(out),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            status = fc.main(["rates", "2026-10-13", *flags])
        return status, out.getvalue()

    def test_a_service_with_no_row_and_an_empty_day_report_and_pass(self) -> None:
        legacy = {"USW2-MP:USW2_InputTokenCount-Units": (0.0004, 0.0013)}
        service = "Claude Sonnet 4.5 (Amazon Bedrock Edition)"
        status, out = self.run_rates(self.usage_response(legacy, service))
        self.assertEqual(status, 0)
        self.assertIn("no RATES_BY_MODEL row bills under this service", out)
        status, out = self.run_rates({"ResultsByTime": []})
        self.assertEqual(status, 0)
        self.assertIn("no Bedrock-edition usage billed", out)

    def test_rates_json_carries_the_derived_rates_and_check(self) -> None:
        status, out = self.run_rates(self.usage_response(self.OPUS_USAGE), "--json")
        self.assertEqual(status, 0)
        entry = json.loads(out)["services"][self.OPUS_SERVICE]
        self.assertAlmostEqual(entry["rates"]["output"], 22.00, places=2)
        self.assertEqual(len(entry["usage_types"]), 4)
        self.assertTrue(entry["check"]["matches"])

    def test_a_malformed_or_paginated_usage_response_exits_two(self) -> None:
        malformed = self.usage_response(self.OPUS_USAGE)
        del malformed["ResultsByTime"][0]["Groups"][0]["Metrics"]["UsageQuantity"]
        paginated = {**self.usage_response(self.OPUS_USAGE), "NextPageToken": "x"}
        for response in (malformed, paginated):
            self.assertEqual(self.run_rates(response)[0], 2)

    def test_rates_exits_one_when_a_row_misses_its_bill(self) -> None:
        drifted = {**self.OPUS_USAGE}
        drifted["USW2-MP:USW2_input_tokens_standard-Units"] = (1.2747, 6.61)
        cases = ((self.OPUS_USAGE, 0), (drifted, 1))
        for usage, status in cases:
            response = self.usage_response(usage)
            out = io.StringIO()
            with (
                mock.patch.object(fc, "aws", return_value=response),
                contextlib.redirect_stdout(out),
            ):
                self.assertEqual(fc.main(["rates", "2026-10-13"]), status)
            self.assertIn("Claude Opus 5.5", out.getvalue())
        with mock.patch.object(fc, "aws", side_effect=fc.AwsError("denied")):
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(fc.main(["rates", "2026-10-13"]), 2)

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


try:
    from zoneinfo import ZoneInfo

    PACIFIC = ZoneInfo("America/Los_Angeles")
except Exception:  # noqa: BLE001 — no tz database, so the DST cases skip
    PACIFIC = None

UTC = datetime.timezone.utc
EDITION = "Claude Opus 5.5 (Amazon Bedrock Edition)"


def hour(text: str) -> datetime.datetime:
    return datetime.datetime.fromisoformat(text).replace(tzinfo=UTC)


def hourly_page(hours: list[tuple[str, float]], token: str | None = None) -> dict:
    page: dict = {
        "ResultsByTime": [
            {
                "TimePeriod": {"Start": f"{start}Z"},
                "Groups": [
                    {
                        "Keys": [EDITION],
                        "Metrics": {"UnblendedCost": {"Amount": str(d)}},
                    },
                    {
                        "Keys": ["Amazon S3"],
                        "Metrics": {"UnblendedCost": {"Amount": "9"}},
                    },
                ],
            }
            for start, d in hours
        ]
    }
    if token:
        page["NextPageToken"] = token
    return page


@unittest.skipIf(PACIFIC is None, "no tz database")
class LocalDayTest(unittest.TestCase):
    def test_windows_keep_each_days_own_offset_across_dst(self) -> None:
        # 2026-11-01 is the fall-back day: 25 hours long in Pacific time.
        windows = fc.local_windows(hour("2026-11-02T17:00:00"), PACIFIC)
        self.assertEqual(len(windows), fc.LOCAL_DAYS + 1)
        self.assertEqual(windows[0][0], "2026-10-26")
        self.assertEqual(windows[0][1], hour("2026-10-26T07:00:00"))
        fall_back = dict((d, (s, e)) for d, s, e in windows)["2026-11-01"]
        # Same-tzinfo subtraction is wall-clock, so compare the instants in UTC.
        start, end = (t.astimezone(UTC) for t in fall_back)
        self.assertEqual(end - start, datetime.timedelta(hours=25))
        self.assertEqual(windows[-1][0], "2026-11-02")
        self.assertEqual(windows[-1][2], hour("2026-11-02T17:00:00"))

    def test_a_timestamp_buckets_under_its_local_date(self) -> None:
        day_of = fc.local_day_of(PACIFIC)
        self.assertEqual(day_of("2026-10-28T03:30:00.000Z"), "2026-10-27")
        self.assertEqual(day_of("2026-10-28T07:00:00.000Z"), "2026-10-28")
        self.assertEqual(day_of("garbage"), "")

    def test_the_walk_meters_by_local_day(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            projects = Path(tmp) / "projects"
            (projects / "seat").mkdir(parents=True)
            line = json.loads(record("msg_late", tmp))
            line["timestamp"] = "2026-10-28T03:00:00.000Z"
            (projects / "seat" / "s.jsonl").write_text(json.dumps(line) + "\n")
            meter = fc.walk(
                projects,
                ["2026-10-27", "2026-10-28"],
                day_of=fc.local_day_of(PACIFIC),
                floor=hour("2026-10-27T07:00:00").timestamp(),
            )
        self.assertIn("seat", fc.day_summary(meter, "2026-10-27")["substrates"])
        self.assertEqual(fc.day_summary(meter, "2026-10-28")["substrates"], {})

    def test_hours_bucket_by_local_start_and_name_the_last_billed(self) -> None:
        hours = fc.parse_hourly(
            hourly_page(
                [
                    ("2026-10-28T06:00:00", 1.5),
                    ("2026-10-28T07:00:00", 2.0),
                    ("2026-10-28T08:00:00", 0.0),
                ]
            )
        )
        days, through = fc.bucket_hours(hours, PACIFIC)
        # The S3 group is not a Bedrock-edition service, so it never counts.
        self.assertEqual(days, {"2026-10-27": 1.5, "2026-10-28": 2.0})
        self.assertEqual(through, hour("2026-10-28T08:00:00"))

    def test_the_hourly_read_pages_on_the_hour_grid(self) -> None:
        pages = [
            hourly_page([("2026-10-28T07:00:00", 1.0)], token="next"),
            hourly_page([("2026-10-28T08:00:00", 2.0)]),
        ]
        with mock.patch.object(fc, "aws", side_effect=pages) as aws:
            hours = fc.fetch_hourly(
                hour("2026-10-28T07:20:00"), hour("2026-10-28T09:20:00")
            )
        self.assertEqual([d for _, d in hours], [1.0, 2.0])
        first, second = (c.args[0] for c in aws.call_args_list)
        # Both ends floored: the running 09:00 hour is never asked for.
        self.assertIn("Start=2026-10-28T07:00:00Z,End=2026-10-28T09:00:00Z", first)
        self.assertEqual(second[-2:], ["--next-page-token", "next"])

    def run_local(self, error: fc.AwsError) -> str:
        out = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp:
            with (
                mock.patch.object(fc.sm, "claude_home", return_value=Path(tmp)),
                mock.patch.object(fc, "aws", side_effect=error),
                contextlib.redirect_stdout(out),
            ):
                rc = fc.local_main(False, PACIFIC, hour("2026-10-29T17:00:00"))
        self.assertEqual(rc, 0)
        return out.getvalue()

    def test_the_billed_column_says_when_hourly_granularity_is_off(self) -> None:
        text = self.run_local(
            fc.AwsError(
                "aws ce get-cost-and-usage exited 254: An error occurred "
                "(AccessDeniedException) when calling the GetCostAndUsage "
                "operation: Hourly data granularity is an opt-in only feature."
            )
        )
        self.assertIn("(PDT), hourly granularity off;", text)
        self.assertIn(
            "2026-10-28 yesterday: billed hourly granularity off | metered $0.00 (0)",
            text,
        )
        self.assertIn("2026-10-29 today so far:", text)
        self.assertIn("2026-10-22 Thu:", text)

    def test_any_other_billed_failure_is_named_and_the_meter_still_prints(self) -> None:
        text = self.run_local(fc.AwsError("aws ce get-cost-and-usage exited 255: x"))
        self.assertIn("billed read failed: aws ce", text)
        self.assertIn("last 7 days:", text)

    def test_the_heading_names_the_hour_billed_through(self) -> None:
        report = {"zone": "PDT", "billed": "ok", "billed_through": "2026-10-29 09:00"}
        self.assertIn(
            "(PDT), billed through 2026-10-29 09:00;", fc.local_heading(report)
        )


if __name__ == "__main__":
    unittest.main()
