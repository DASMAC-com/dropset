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


class GroupForColor(unittest.TestCase):
    def test_maps_state_hexes(self):
        self.assertEqual(ir._group_for_color("3a2c08"), "yellow")
        self.assertEqual(ir._group_for_color("080c2a"), "green")
        self.assertEqual(ir._group_for_color("082a0c"), "green")
        self.assertEqual(ir._group_for_color("16191e"), "neutral")
        self.assertEqual(ir._group_for_color(""), "neutral")


if __name__ == "__main__":
    unittest.main()
