#!/usr/bin/env python3
"""Unit tests for iterm-reorder.py's FIFO ordering (stdlib unittest; no pytest).

The module filename has a hyphen (to match the iterm-* family) so it can't be
imported by name; load it by path. Its `iterm2` import is guarded, so the pure
ordering logic imports fine without the package installed.
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "iterm_reorder", Path(__file__).with_name("iterm-reorder.py")
)
ir = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(ir)


class LoadPalette(unittest.TestCase):
    """The palette is read from iterm-colors.sh, not copied. The duplication it
    replaced was silent: an unrecognized hex falls through to "neutral", so a
    recolor left the colors right and killed attention ordering outright."""

    def _write(self, body):
        import tempfile

        tmp = tempfile.NamedTemporaryFile(
            "w", suffix=".sh", delete=False, encoding="utf-8"
        )
        tmp.write(body)
        tmp.close()
        self.addCleanup(lambda: Path(tmp.name).unlink(missing_ok=True))
        return Path(tmp.name)

    def test_reads_the_committed_palette(self):
        palette, from_file = ir.load_palette()
        self.assertTrue(from_file, "the committed iterm-colors.sh must be found")
        self.assertEqual(palette["yellow"], {"3a2c08"})
        self.assertEqual(palette["green"], {"080c2a", "082a0c"})

    def test_a_recolored_palette_is_picked_up(self):
        """The whole point: new hexes must classify, not fall through."""
        path = self._write(
            'STATE_NEUTRAL="111111"\n'
            'STATE_REPLY="222222"\n'
            'STATE_PERMISSION="333333"\n'
            'STATE_MARK="444444"\n'
        )
        palette, from_file = ir.load_palette(path)
        self.assertTrue(from_file)
        self.assertEqual(ir._group_for_color("333333", palette), "yellow")
        self.assertEqual(ir._group_for_color("222222", palette), "green")
        self.assertEqual(ir._group_for_color("444444", palette), "green")
        self.assertEqual(ir._group_for_color("111111", palette), "neutral")
        # And the OLD hexes must no longer classify — that is what "read the
        # palette" buys over "keep the copies in sync".
        self.assertEqual(ir._group_for_color("3a2c08", palette), "neutral")

    def test_a_partial_palette_falls_back_whole(self):
        """Half a palette would silently demote the missing state to neutral,
        which is the failure mode being removed — so fall back entirely."""
        path = self._write('STATE_PERMISSION="333333"\n')
        palette, from_file = ir.load_palette(path)
        self.assertFalse(from_file)
        self.assertEqual(palette["yellow"], {"3a2c08"})

    def test_a_missing_palette_file_falls_back(self):
        palette, from_file = ir.load_palette(Path("/nonexistent/iterm-colors.sh"))
        self.assertFalse(from_file)
        self.assertEqual(palette["green"], {"080c2a", "082a0c"})

    def test_group_lookup_is_case_and_whitespace_tolerant(self):
        self.assertEqual(ir._group_for_color(" 3A2C08\n"), "yellow")


class PlanOrder(unittest.TestCase):
    def setUp(self):
        self.seq, self.last_group, self.counter = {}, {}, 0

    def step(self, entries):
        order, self.counter = ir.plan_order(
            entries, self.seq, self.last_group, self.counter
        )
        ordered = [entries[i] for i in order]
        return [t for t, _ in ordered], ordered

    def test_groups_yellow_then_green_fifo(self):
        ids, _ = self.step(
            [("A", "yellow"), ("B", "yellow"), ("C", "green"), ("D", "green")]
        )
        self.assertEqual(ids, ["A", "B", "C", "D"])

    def test_new_yellow_goes_to_back_of_its_group(self):
        _, ordered = self.step(
            [("A", "yellow"), ("B", "yellow"), ("C", "green"), ("D", "green")]
        )
        # A fresh yellow lands after the yellows, before the greens.
        ids, _ = self.step(ordered + [("E", "yellow")])
        self.assertEqual(ids, ["A", "B", "E", "C", "D"])

    def test_cleared_oldest_drops_out_and_next_slides_to_front(self):
        _, ordered = self.step(
            [("A", "yellow"), ("B", "yellow"), ("C", "green"), ("D", "green")]
        )
        ids, _ = self.step([(t, "neutral" if t == "A" else g) for t, g in ordered])
        self.assertEqual(ids, ["B", "C", "D", "A"])

    def test_cleared_tab_sinks_below_all_attention(self):
        # Clearing the front tab must put the next attention tab at position 1.
        _, ordered = self.step([("A", "yellow"), ("C", "green"), ("D", "green")])
        ids, _ = self.step([(t, "neutral" if t == "A" else g) for t, g in ordered])
        self.assertEqual(ids[0], "C")  # next attention item leads
        self.assertEqual(ids[-1], "A")  # cleared tab sank to the back

    def test_reentering_attention_gets_a_fresh_back_slot(self):
        self.step([("A", "yellow"), ("B", "yellow")])
        self.step([("A", "neutral"), ("B", "yellow")])  # A cleared
        # A re-enters yellow: it must queue behind B (which kept its slot).
        ids, _ = self.step([("A", "yellow"), ("B", "yellow")])
        self.assertEqual(ids, ["B", "A"])


class PlanningTab(unittest.TestCase):
    """A resting planning tab sorts last, so Command-9 always reaches it; one
    that needs attention queues in its group like any other tab."""

    def setUp(self):
        self.seq, self.last_group, self.counter = {}, {}, 0

    def step(self, tabs, plan_group):
        # `tabs` is the non-planning tabs; P is the planning tab, listed first so
        # every placement below is the reorderer's doing, not the input order.
        entries = [("P", ir.effective_group(plan_group, ir.PLAN_ROLE))] + tabs
        order, self.counter = ir.plan_order(
            entries, self.seq, self.last_group, self.counter
        )
        return [entries[i][0] for i in order]

    def test_resting_planning_tab_is_rightmost_alone(self):
        ids = self.step([("A", "neutral"), ("B", "neutral")], "neutral")
        self.assertEqual(ids, ["A", "B", "P"])

    def test_resting_planning_tab_is_rightmost_behind_attention(self):
        ids = self.step([("A", "neutral"), ("B", "green"), ("C", "yellow")], "neutral")
        self.assertEqual(ids, ["C", "B", "A", "P"])

    def test_a_planning_ask_surfaces_at_position_one(self):
        ids = self.step([("A", "neutral"), ("B", "neutral")], "green")
        self.assertEqual(ids[0], "P")

    def test_a_planning_ask_queues_fifo_behind_older_attention(self):
        self.step([("A", "green"), ("B", "neutral")], "neutral")
        ids = self.step([("A", "green"), ("B", "neutral")], "green")
        self.assertEqual(ids, ["A", "P", "B"])

    def test_a_cleared_planning_tab_returns_to_the_rightmost_slot(self):
        tabs = [("A", "neutral"), ("B", "yellow")]
        self.step(tabs, "yellow")
        ids = self.step(tabs, "neutral")
        self.assertEqual(ids, ["B", "A", "P"])

    def test_the_role_is_found_on_any_pane_of_a_split_tab(self):
        import asyncio

        def session(role):
            class Session:
                async def async_get_variable(self, name):
                    return role if name == ir.ROLE_VARIABLE else None

            return Session()

        split = type("Tab", (), {})()
        split.sessions = [session(None), session(ir.PLAN_ROLE)]
        plain = type("Tab", (), {})()
        plain.sessions = [session(None), session("task")]
        self.assertEqual(asyncio.run(ir._tab_role(split)), ir.PLAN_ROLE)
        self.assertIsNone(asyncio.run(ir._tab_role(plain)))

    def test_only_the_marked_role_is_special(self):
        self.assertEqual(ir.effective_group("neutral", None), "neutral")
        self.assertEqual(ir.effective_group("neutral", ""), "neutral")
        self.assertEqual(ir.effective_group("neutral", "task"), "neutral")
        self.assertEqual(ir.effective_group("green", ir.PLAN_ROLE), "green")
        self.assertEqual(ir.effective_group("yellow", ir.PLAN_ROLE), "yellow")


class GroupForColor(unittest.TestCase):
    def test_maps_state_hexes(self):
        self.assertEqual(ir._group_for_color("3a2c08"), "yellow")
        self.assertEqual(ir._group_for_color("080c2a"), "green")
        self.assertEqual(ir._group_for_color("082a0c"), "green")
        self.assertEqual(ir._group_for_color("16191e"), "neutral")
        self.assertEqual(ir._group_for_color(""), "neutral")


if __name__ == "__main__":
    unittest.main()
