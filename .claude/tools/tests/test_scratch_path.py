"""Stdlib ``unittest`` tests for the shared-temp-root guard."""

import io
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import lens_preamble
import review_diff
from scratch_path import shared_root_refusal


class SharedRootRefusalTests(unittest.TestCase):
    def test_a_file_directly_under_a_shared_root_is_refused(self):
        for out in (
            "/tmp/lens-completeness.md",
            "/private/tmp/review-diff.txt",
            "/var/tmp/brief.md",
            str(Path(tempfile.gettempdir()) / "facts.md"),
        ):
            with self.subTest(out=out):
                reason = shared_root_refusal(out)
                self.assertIsNotNone(reason)
                self.assertIn("session-private", reason)

    def test_a_session_keyed_subdirectory_is_allowed(self):
        for out in ("/tmp/eng-123/review-diff.txt", "/private/tmp/s/x/brief.md"):
            with self.subTest(out=out):
                self.assertIsNone(shared_root_refusal(out))

    def test_a_path_outside_any_temp_root_is_allowed(self):
        self.assertIsNone(shared_root_refusal("/Users/someone/scratch/brief.md"))


class ToolWiringTests(unittest.TestCase):
    """Both writers refuse before writing anything."""

    def test_lens_preamble_refuses_a_shared_root_out(self):
        with self.assertRaises(lens_preamble.LensPreambleError):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                lens_preamble.run(
                    ["lens_preamble.py", "--out", "/tmp/brief.md", "--no-facts"]
                )

    def test_review_diff_refuses_a_shared_root_out(self):
        with self.assertRaises(review_diff.ReviewDiffError):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                review_diff.run(
                    ["review_diff.py", "--out", "/tmp/review-diff.txt", "--no-fetch"]
                )


if __name__ == "__main__":
    unittest.main()
