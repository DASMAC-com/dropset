#!/usr/bin/env python3
"""Tests for `render_review.py`.

Split deliberately: the ordering, discovery, duplicate and CLI-shape logic is
stdlib-only and always runs, because that is what CI's lint job can execute
(its Python install is `pre-commit` and nothing else). The pixel modes are
guarded by `HAS_PIL` so a host without Pillow reports skips rather than errors.
"""

from __future__ import annotations

import io
import tempfile
import unittest
import zipfile
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import render_review as rr

try:
    from PIL import Image

    HAS_PIL = True
except ImportError:  # pragma: no cover - depends on the host
    HAS_PIL = False


REL_TMPL = (
    '<?xml version="1.0"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/'
    'relationships">{rels}</Relationships>'
)
SLIDE_TMPL = (
    '<?xml version="1.0"?>'
    '<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'
    ' xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
    ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/'
    'relationships">{blips}</p:sld>'
)


def png_bytes(width=40, height=30, color=(200, 30, 30), ink=None):
    """A tiny PNG, optionally with a darker rectangle as `ink`."""
    img = Image.new("RGB", (width, height), color)
    if ink:
        left, top, right, bottom = ink
        for x in range(left, right):
            for y in range(top, bottom):
                img.putpixel((x, y), (10, 10, 10))
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def build_pptx(path: Path, slide_media: list[list[str]], media: dict[str, bytes]):
    """A minimal .pptx: `slide_media[i]` names the media parts slide i embeds."""
    with zipfile.ZipFile(path, "w") as zf:
        for name, data in media.items():
            zf.writestr(f"ppt/media/{name}", data)
        for index, names in enumerate(slide_media, start=1):
            blips = "".join(
                f'<a:blip r:embed="rId{n + 1}"/>' for n in range(len(names))
            )
            zf.writestr(f"ppt/slides/slide{index}.xml", SLIDE_TMPL.format(blips=blips))
            rels = "".join(
                f'<Relationship Id="rId{n + 1}" Target="../media/{name}"/>'
                for n, name in enumerate(names)
            )
            zf.writestr(
                f"ppt/slides/_rels/slide{index}.xml.rels",
                REL_TMPL.format(rels=rels),
            )


class NaturalKeyTests(unittest.TestCase):
    def test_slide_ten_sorts_after_slide_two(self):
        """Plain string order puts slide10 second — a deck silently reordered at
        page 10 is a nasty way to discover that."""
        names = ["slide10.xml", "slide2.xml", "slide1.xml"]
        self.assertEqual(
            sorted(names, key=rr.natural_key),
            ["slide1.xml", "slide2.xml", "slide10.xml"],
        )


class ResolveMediaTests(unittest.TestCase):
    def test_a_relative_target_resolves_against_the_slides_dir(self):
        self.assertEqual(
            rr._resolve_media("../media/image3.png"), "ppt/media/image3.png"
        )

    def test_an_absolute_target_is_taken_as_a_part_name(self):
        self.assertEqual(
            rr._resolve_media("/ppt/media/image3.png"), "ppt/media/image3.png"
        )

    def test_an_empty_target_is_none(self):
        self.assertIsNone(rr._resolve_media(""))


class SlideOrderTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def test_pages_follow_slide_order_not_media_numbering(self):
        """The whole reason this walks the rels. Media numbering follows first
        use, so a deck reusing a background reorders under a naive sort."""
        deck = self.root / "deck.pptx"
        build_pptx(
            deck,
            slide_media=[["image9.png"], ["image1.png"], ["image4.png"]],
            media={
                "image1.png": b"one",
                "image4.png": b"four",
                "image9.png": b"nine",
            },
        )
        pages = rr.load_pages(deck)
        self.assertEqual(
            [label for label, _ in pages],
            ["image9.png", "image1.png", "image4.png"],
        )

    def test_a_media_part_reused_across_slides_is_not_repeated(self):
        deck = self.root / "deck.pptx"
        build_pptx(
            deck,
            slide_media=[["bg.png"], ["bg.png", "hero.png"]],
            media={"bg.png": b"bg", "hero.png": b"hero"},
        )
        pages = rr.load_pages(deck)
        self.assertEqual([label for label, _ in pages], ["bg.png", "hero.png"])

    def test_a_deck_without_rels_falls_back_to_media_order(self):
        """Best-effort beats no answer for a deck built by another tool."""
        deck = self.root / "deck.pptx"
        with zipfile.ZipFile(deck, "w") as zf:
            zf.writestr("ppt/media/image2.png", b"two")
            zf.writestr("ppt/media/image1.png", b"one")
        pages = rr.load_pages(deck)
        self.assertEqual([label for label, _ in pages], ["image1.png", "image2.png"])

    def test_a_directory_of_pages_sorts_naturally(self):
        pages_dir = self.root / "pages"
        pages_dir.mkdir()
        for name in ("page10.png", "page2.png", "page1.png"):
            (pages_dir / name).write_bytes(b"x")
        pages = rr.load_pages(pages_dir)
        self.assertEqual(
            [label for label, _ in pages], ["page1.png", "page2.png", "page10.png"]
        )

    def test_a_directory_ignores_non_images(self):
        pages_dir = self.root / "pages"
        pages_dir.mkdir()
        (pages_dir / "page1.png").write_bytes(b"x")
        (pages_dir / "notes.md").write_text("not a page", encoding="utf-8")
        pages = rr.load_pages(pages_dir)
        self.assertEqual([label for label, _ in pages], ["page1.png"])

    def test_an_empty_directory_errors(self):
        empty = self.root / "empty"
        empty.mkdir()
        with self.assertRaises(rr.RenderReviewError):
            rr.load_pages(empty)

    def test_a_missing_source_errors(self):
        with self.assertRaises(rr.RenderReviewError):
            rr.load_pages(self.root / "nope.pptx")

    def test_a_non_pptx_file_errors(self):
        stray = self.root / "notes.md"
        stray.write_text("hi", encoding="utf-8")
        with self.assertRaises(rr.RenderReviewError):
            rr.load_pages(stray)

    def test_a_corrupt_pptx_errors_rather_than_raising_zipfile(self):
        broken = self.root / "deck.pptx"
        broken.write_bytes(b"not a zip")
        with self.assertRaises(rr.RenderReviewError):
            rr.load_pages(broken)


class DuplicateTests(unittest.TestCase):
    def test_identical_pages_are_grouped(self):
        """A repeated export page looks fine one page at a time."""
        pages = [("a", b"x"), ("b", b"y"), ("c", b"x")]
        self.assertEqual(rr.duplicate_groups(pages), [[1, 3]])

    def test_distinct_pages_group_nothing(self):
        pages = [("a", b"x"), ("b", b"y")]
        self.assertEqual(rr.duplicate_groups(pages), [])


@unittest.skipUnless(HAS_PIL, "Pillow is not installed")
class MeasureTests(unittest.TestCase):
    def test_a_flat_page_reads_as_blank(self):
        m = rr.measure_page(png_bytes())
        self.assertTrue(m["blank"])
        self.assertIsNone(m["ink_box"])

    def test_ink_box_and_margins(self):
        data = png_bytes(width=100, height=100, ink=(10, 20, 90, 60))
        m = rr.measure_page(data)
        self.assertFalse(m["blank"])
        self.assertEqual(m["ink_box"], [10, 20, 90, 60])
        self.assertEqual(
            m["margins"], {"left": 10, "top": 20, "right": 10, "bottom": 40}
        )
        self.assertEqual(m["margins_pct"]["bottom"], 40.0)

    def test_the_report_names_mixed_page_sizes(self):
        pages = [
            ("a.png", png_bytes(width=100, height=50)),
            ("b.png", png_bytes(width=80, height=50)),
        ]
        report = rr.format_measurements(pages)
        self.assertIn("mixed page sizes", report)

    def test_the_report_names_identical_pages(self):
        same = png_bytes(width=60, height=40)
        report = rr.format_measurements([("a.png", same), ("b.png", same)])
        self.assertIn("identical pages: p1, p2", report)

    def test_the_report_says_so_when_all_pages_differ(self):
        pages = [
            ("a.png", png_bytes(width=60, height=40, ink=(1, 1, 5, 5))),
            ("b.png", png_bytes(width=60, height=40, ink=(2, 2, 9, 9))),
        ]
        report = rr.format_measurements(pages)
        self.assertIn("distinct pages: all", report)
        self.assertNotIn("WARNING", report)


@unittest.skipUnless(HAS_PIL, "Pillow is not installed")
class DownscaleTests(unittest.TestCase):
    def test_a_wide_page_is_scaled_down_keeping_aspect(self):
        img = rr._downscaled(png_bytes(width=1000, height=500), 200)
        self.assertEqual((img.width, img.height), (200, 100))

    def test_a_page_narrower_than_the_target_is_left_alone(self):
        """Upscaling buys tokens and no detail."""
        img = rr._downscaled(png_bytes(width=100, height=50), 400)
        self.assertEqual((img.width, img.height), (100, 50))


@unittest.skipUnless(HAS_PIL, "Pillow is not installed")
class MontageTests(unittest.TestCase):
    def test_grid_geometry_follows_the_column_count(self):
        pages = [(f"p{i}.png", png_bytes(width=200, height=100)) for i in range(5)]
        sheet = rr.build_montage(pages, cols=2, cell_width=100)
        # 2 cols x 100 + 3 gutters of 12
        self.assertEqual(sheet.width, 2 * 100 + 3 * 12)
        # 3 rows of (50 cell + 16 label) + 4 gutters of 12
        self.assertEqual(sheet.height, 3 * (50 + 16) + 4 * 12)

    def test_a_single_column_stacks(self):
        pages = [(f"p{i}.png", png_bytes(width=200, height=100)) for i in range(3)]
        sheet = rr.build_montage(pages, cols=1, cell_width=100)
        self.assertEqual(sheet.width, 100 + 2 * 12)


class CliTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def _capture(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = rr.run(argv)
        return code, out.getvalue(), err.getvalue()

    def _deck(self, pages=3):
        deck = self.root / "deck.pptx"
        media = {}
        slide_media = []
        for i in range(1, pages + 1):
            name = f"image{i}.png"
            media[name] = (
                png_bytes(width=200, height=100, ink=(i, i, i + 20, i + 20))
                if HAS_PIL
                else bytes([i])
            )
            slide_media.append([name])
        build_pptx(deck, slide_media, media)
        return deck

    def test_zero_columns_is_refused(self):
        with self.assertRaises(rr.RenderReviewError):
            rr.run(["render_review.py", str(self._deck()), "--montage", "--cols", "0"])

    @unittest.skipUnless(HAS_PIL, "Pillow is not installed")
    def test_measure_prints_a_line_per_page_and_a_count(self):
        code, out, _ = self._capture(
            ["render_review.py", str(self._deck(3)), "--measure"]
        )
        self.assertEqual(code, 0)
        self.assertIn("p1", out)
        self.assertIn("p3", out)
        self.assertIn("pages: 3", out)

    @unittest.skipUnless(HAS_PIL, "Pillow is not installed")
    def test_montage_writes_one_sheet_and_prints_its_path(self):
        out_dir = self.root / "out"
        code, out, err = self._capture(
            [
                "render_review.py",
                str(self._deck(4)),
                "--montage",
                "--out",
                str(out_dir),
            ]
        )
        self.assertEqual(code, 0)
        written = Path(out.strip())
        self.assertTrue(written.is_file())
        self.assertIn("contact-sheet", written.name)
        self.assertIn("4 page(s)", err)

    @unittest.skipUnless(HAS_PIL, "Pillow is not installed")
    def test_montage_is_the_default_mode(self):
        """Defaulting to the cheap sweep is what makes it the easy path."""
        out_dir = self.root / "out"
        code, out, _ = self._capture(
            ["render_review.py", str(self._deck(2)), "--out", str(out_dir)]
        )
        self.assertEqual(code, 0)
        self.assertIn("contact-sheet", Path(out.strip()).name)

    @unittest.skipUnless(HAS_PIL, "Pillow is not installed")
    def test_page_writes_one_downscaled_page(self):
        out_dir = self.root / "out"
        code, out, err = self._capture(
            [
                "render_review.py",
                str(self._deck(3)),
                "--page",
                "2",
                "--width",
                "120",
                "--out",
                str(out_dir),
            ]
        )
        self.assertEqual(code, 0)
        self.assertIn("page 2/3", err)
        self.assertIn("-p2-", Path(out.strip()).name)

    @unittest.skipUnless(HAS_PIL, "Pillow is not installed")
    def test_a_page_out_of_range_says_how_many_there_are(self):
        with self.assertRaises(rr.RenderReviewError) as ctx:
            rr.run(["render_review.py", str(self._deck(3)), "--page", "9"])
        self.assertIn("the deck has 3", str(ctx.exception))

    def test_the_modes_are_mutually_exclusive(self):
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            rr.run(["render_review.py", str(self._deck(1)), "--measure", "--montage"])


if __name__ == "__main__":
    unittest.main()
