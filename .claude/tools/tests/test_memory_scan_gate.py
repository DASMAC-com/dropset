"""Stdlib ``unittest`` tests for the memory-scan cadence gate.

Covers the content signature over the ``*.md`` store and the pure ``decide``
rule (no-marker, changed-store, within-interval, elapsed, bad-timestamp), plus
argument parsing. Run via the repo's ``make tools-tests``.
"""

import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

from memory_scan_gate import (
    MemoryScanGateError,
    _parse_args,
    decide,
    store_signature,
)

NOW = datetime(2026, 7, 15, 12, 0, 0, tzinfo=timezone.utc)


class StoreSignatureTests(unittest.TestCase):
    def test_empty_or_absent_store_is_sentinel(self):
        with tempfile.TemporaryDirectory() as d:
            empty = Path(d) / "memory"
            self.assertEqual(store_signature(empty), "empty")  # absent
            empty.mkdir()
            self.assertEqual(store_signature(empty), "empty")  # present but no md

    def test_signature_changes_on_add_and_edit(self):
        with tempfile.TemporaryDirectory() as d:
            mem = Path(d) / "memory"
            mem.mkdir()
            (mem / "a.md").write_text("one", encoding="utf-8")
            sig1 = store_signature(mem)
            (mem / "b.md").write_text("two", encoding="utf-8")
            sig2 = store_signature(mem)
            self.assertNotEqual(sig1, sig2)  # add changed it
            (mem / "a.md").write_text("one!", encoding="utf-8")
            sig3 = store_signature(mem)
            self.assertNotEqual(sig2, sig3)  # edit changed it

    def test_non_md_sidecar_does_not_affect_signature(self):
        with tempfile.TemporaryDirectory() as d:
            mem = Path(d) / "memory"
            mem.mkdir()
            (mem / "a.md").write_text("one", encoding="utf-8")
            sig1 = store_signature(mem)
            (mem / ".marker.json").write_text("{}", encoding="utf-8")
            self.assertEqual(sig1, store_signature(mem))


class DecideTests(unittest.TestCase):
    def test_no_marker_scans(self):
        scan, reason = decide("sig", None, NOW)
        self.assertTrue(scan)
        self.assertIn("no prior scan", reason)

    def test_changed_signature_scans(self):
        marker = {"signature": "old", "last_scan": NOW.isoformat()}
        scan, reason = decide("new", marker, NOW)
        self.assertTrue(scan)
        self.assertIn("changed", reason)

    def test_unchanged_within_interval_skips(self):
        marker = {
            "signature": "sig",
            "last_scan": (NOW - timedelta(hours=2)).isoformat(),
        }
        scan, _ = decide("sig", marker, NOW, min_interval_hours=20)
        self.assertFalse(scan)

    def test_unchanged_past_interval_scans(self):
        marker = {
            "signature": "sig",
            "last_scan": (NOW - timedelta(hours=25)).isoformat(),
        }
        scan, reason = decide("sig", marker, NOW, min_interval_hours=20)
        self.assertTrue(scan)
        self.assertIn("since last scan", reason)

    def test_bad_timestamp_scans(self):
        scan, reason = decide("sig", {"signature": "sig", "last_scan": "nonsense"}, NOW)
        self.assertTrue(scan)
        self.assertIn("timestamp", reason)


class ParseArgsTests(unittest.TestCase):
    def test_check_with_dir(self):
        mode, path, hours = _parse_args(["check", "/tmp/mem"])
        self.assertEqual(mode, "check")
        self.assertEqual(path, Path("/tmp/mem"))
        self.assertEqual(hours, 20.0)

    def test_min_interval_override(self):
        _, _, hours = _parse_args(["check", "/tmp/mem", "--min-interval-hours", "6"])
        self.assertEqual(hours, 6.0)

    def test_missing_dir_raises(self):
        with self.assertRaises(MemoryScanGateError):
            _parse_args(["check"])

    def test_bad_mode_raises(self):
        with self.assertRaises(MemoryScanGateError):
            _parse_args(["frobnicate", "/tmp/mem"])


if __name__ == "__main__":
    unittest.main()
