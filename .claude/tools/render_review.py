#!/usr/bin/env python3
# cspell:word OOXML
# cspell:word getbbox
# cspell:word getpixel
# cspell:word LANCZOS
"""Review rendered deck output without paying for it at print resolution.

Two measured sessions spent **882.7k of `Read` (~98% of all tool-result cost)**
and **~1.1M (~97%)** on rendered deck pages, one page re-read four times for
~321k combined. Both followed every rule in
``docs/conventions/context-economy.md``; the cost sat in a class that page's
guidance only reached obliquely, because a deck export arrives *many pages at
once* and at *print* resolution.

The insight the two sessions share: every judgment either of them actually made
was about **layout** — does the page overflow, does the headline wrap, is the
footer crowded — and each of those is answered identically by a ~1200px copy,
or by a number, for roughly a tenth of the tokens. Capture resolution stays a
product decision (``decks/scripts/capture.mjs`` sets 2x because 3840x2160 is
exactly a 4K projector); the copy you *look at* is the thing to shrink.

So this tool serves three questions, cheapest first:

``--measure``
    Numbers, no image. Page dimensions, the ink bounding box, the margin on
    each side as px and as a percentage of the page, and a duplicate-page
    check. A few hundred tokens. In one session the cheap checks did the real
    work and *disproved* two hypotheses eyeballing had suggested — an "extra
    export margin" that turned out to be the deck's own layout, and a padding
    edit measurement showed was a no-op.

``--montage``
    One contact sheet for the whole deck. The recurring question "did any page
    clip?" collapses from N full-page reads to a single grid read.

``--page N``
    One page, downscaled. For when the montage shows something worth a closer
    look. Never read the native render for this.

Input is either the exported ``.pptx`` (``decks/out/*.pptx``) or a directory of
page PNGs. For a ``.pptx`` the page order is taken from ``ppt/slides/slideN.xml``
and each slide's rels, **not** from ``ppt/media`` filename order — media
numbering follows first use, which coincides with slide order only while every
slide happens to carry exactly one new image.

Pillow is imported lazily and only by the modes that need pixels, so the module
imports (and its ordering/CLI tests run) on a host without it — CI's lint job
installs `pre-commit` and nothing else. On a host without Pillow the pixel modes
fail with one clear line naming the install.

Usage::

    python3 .claude/tools/render_review.py decks/out/deck.pptx --measure
    python3 .claude/tools/render_review.py decks/out/deck.pptx --montage
    python3 .claude/tools/render_review.py decks/out/deck.pptx --page 7
    python3 .claude/tools/render_review.py decks/out --montage --cols 3

Written images go to a temp directory (like ``run_quiet.py``'s logs) and the
path is printed, so nothing lands in the repo and the caller `Read`s one small
file. A Python skill-tool under ``.claude/tools/`` — deliberately **not** a
Cargo workspace member (see ``CLAUDE.md`` -> "Skill tooling"). Tests live in
``tests/test_render_review.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import re
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

# Where written review copies go. Mirrors `run_quiet.py`: out of the repo, and
# stable enough that a caller can find yesterday's sheet.
OUT_ROOT = Path(tempfile.gettempdir()) / "claude-render-review"

# Default width of a single reviewed page. Chosen because every layout question
# these sessions asked is legible here, and it is ~1/10 the tokens of a 3840px
# render.
DEFAULT_PAGE_WIDTH = 1200

# Default width of one montage cell. Small enough that a 12-page grid is one
# cheap read, large enough that a clipped element or a wrapped headline shows.
DEFAULT_CELL_WIDTH = 420

DEFAULT_COLS = 4

# Namespaces in the OOXML parts this tool reads.
NS_REL = "http://schemas.openxmlformats.org/package/2006/relationships"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"

IMAGE_SUFFIXES = (".png", ".jpg", ".jpeg")


class RenderReviewError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def _require_pillow():
    """Import Pillow at use-site, with an actionable error if it is absent."""
    try:
        from PIL import Image, ImageChops, ImageDraw
    except ImportError as e:  # pragma: no cover - depends on the host
        raise RenderReviewError(
            "every mode of this tool needs Pillow, which is not installed "
            "here — `python3 -m pip install Pillow`. (The page-ordering and "
            "duplicate-detection logic is dependency-free and unit-tested "
            "without it, but no CLI mode stops short of touching pixels.)"
        ) from e
    return Image, ImageChops, ImageDraw


def natural_key(name: str):
    """Sort ``slide2`` before ``slide10`` — plain string order gets this wrong,
    and a deck silently reordered at page 10 is a nasty way to find out."""
    return [int(part) if part.isdigit() else part for part in re.split(r"(\d+)", name)]


def slide_media_order(zf: zipfile.ZipFile) -> list[str]:
    """Media part names in slide order, read from each slide's relationships.

    ``ppt/media`` numbering follows *first use*, which matches slide order only
    while every slide contributes exactly one new image. A deck that reuses a
    background, or carries a logo on some slides, breaks that assumption
    silently — hence walking the rels instead.
    """
    slides = sorted(
        (n for n in zf.namelist() if re.fullmatch(r"ppt/slides/slide\d+\.xml", n)),
        key=natural_key,
    )
    ordered: list[str] = []
    seen: set[str] = set()
    for slide in slides:
        rels_name = f"ppt/slides/_rels/{Path(slide).name}.rels"
        if rels_name not in zf.namelist():
            continue
        try:
            root = ET.fromstring(zf.read(rels_name))
        except ET.ParseError:
            continue
        # Preserve the order the slide's own XML embeds them in, so a slide with
        # several images reads left-to-right as authored.
        embeds = _embed_ids(zf, slide)
        targets = {
            rel.get("Id"): rel.get("Target", "")
            for rel in root.findall(f"{{{NS_REL}}}Relationship")
        }
        for rid in embeds:
            target = targets.get(rid, "")
            if not target:
                continue
            part = _resolve_media(target)
            if part and part in zf.namelist() and part not in seen:
                seen.add(part)
                ordered.append(part)
    return ordered


def _embed_ids(zf: zipfile.ZipFile, slide_name: str) -> list[str]:
    """Relationship ids the slide embeds, in document order."""
    try:
        root = ET.fromstring(zf.read(slide_name))
    except (KeyError, ET.ParseError):
        return []
    ids = []
    for el in root.iter():
        rid = el.get(f"{{{NS_R}}}embed")
        if rid and rid not in ids:
            ids.append(rid)
    return ids


def _resolve_media(target: str) -> str | None:
    """Turn a slide-relative rel target into a zip part name."""
    if not target:
        return None
    cleaned = target.replace("\\", "/")
    if cleaned.startswith("/"):
        cleaned = cleaned.lstrip("/")
        return cleaned
    # Targets are written relative to `ppt/slides/`, so `../media/image1.png`.
    parts: list[str] = []
    for segment in f"ppt/slides/{cleaned}".split("/"):
        if segment == "..":
            if parts:
                parts.pop()
        elif segment not in ("", "."):
            parts.append(segment)
    return "/".join(parts)


def load_pages(source: Path) -> list[tuple[str, bytes]]:
    """``(label, image bytes)`` per page, in deck order.

    Accepts the exported ``.pptx`` or a directory of page images.
    """
    if source.is_dir():
        files = sorted(
            (
                p
                for p in source.iterdir()
                if p.is_file() and p.suffix.lower() in IMAGE_SUFFIXES
            ),
            key=lambda p: natural_key(p.name),
        )
        if not files:
            raise RenderReviewError(f"no page images found in {source}")
        return [(p.name, p.read_bytes()) for p in files]

    if not source.is_file():
        raise RenderReviewError(f"no such file or directory: {source}")

    if source.suffix.lower() != ".pptx":
        raise RenderReviewError(
            f"expected a .pptx or a directory of page images, got {source.name}"
        )

    try:
        with zipfile.ZipFile(source) as zf:
            parts = slide_media_order(zf)
            if not parts:
                # Fall back to media order rather than failing: a deck built by
                # another tool may not carry the rels this expects, and a
                # best-effort ordering beats no answer.
                parts = sorted(
                    (
                        n
                        for n in zf.namelist()
                        if n.startswith("ppt/media/")
                        and Path(n).suffix.lower() in IMAGE_SUFFIXES
                    ),
                    key=natural_key,
                )
            if not parts:
                raise RenderReviewError(f"no page images inside {source.name}")
            return [(Path(p).name, zf.read(p)) for p in parts]
    except zipfile.BadZipFile as e:
        raise RenderReviewError(f"{source.name} is not a readable .pptx") from e


def measure_page(data: bytes) -> dict:
    """Dimensions, ink box, and per-side margins for one page.

    The margins are the measurement that settles the recurring questions —
    "does this overflow", "is the footer crowded", "did the padding edit do
    anything" — for a few hundred tokens rather than a full-page read.
    """
    Image, ImageChops, _ = _require_pillow()
    with Image.open(io.BytesIO(data)) as img:
        rgb = img.convert("RGB")
        width, height = rgb.size
        # Treat the top-left pixel as the page background. Deck pages are
        # full-bleed on a flat backdrop, so this is reliable here and cheap;
        # a photo-backed page just reports a full-page ink box, which reads
        # correctly as "ink reaches the edges".
        background = rgb.getpixel((0, 0))
        solid = Image.new("RGB", rgb.size, background)
        bbox = ImageChops.difference(rgb, solid).getbbox()

    if bbox is None:
        return {
            "width": width,
            "height": height,
            "blank": True,
            "ink_box": None,
            "margins": None,
        }

    left, top, right, bottom = bbox
    margins = {
        "left": left,
        "top": top,
        "right": width - right,
        "bottom": height - bottom,
    }
    return {
        "width": width,
        "height": height,
        "blank": False,
        "ink_box": [left, top, right, bottom],
        "margins": margins,
        "margins_pct": {
            "left": round(100 * margins["left"] / width, 2),
            "top": round(100 * margins["top"] / height, 2),
            "right": round(100 * margins["right"] / width, 2),
            "bottom": round(100 * margins["bottom"] / height, 2),
        },
    }


def duplicate_groups(pages: list[tuple[str, bytes]]) -> list[list[int]]:
    """1-based page indices that share identical bytes.

    A deck whose export repeated a page looks fine one page at a time; this is
    the check that catches it without reading any of them.
    """
    by_hash: dict[str, list[int]] = {}
    for index, (_, data) in enumerate(pages, start=1):
        digest = hashlib.sha256(data).hexdigest()
        by_hash.setdefault(digest, []).append(index)
    return [group for group in by_hash.values() if len(group) > 1]


def _downscaled(data: bytes, width: int):
    """One page as a Pillow image, no wider than ``width``."""
    Image, _, _ = _require_pillow()
    img = Image.open(io.BytesIO(data))
    img = img.convert("RGB")
    if img.width > width:
        height = max(1, round(img.height * width / img.width))
        img = img.resize((width, height), Image.LANCZOS)
    return img


def build_montage(pages: list[tuple[str, bytes]], cols: int, cell_width: int):
    """A labelled contact sheet: the whole deck as one cheap read."""
    Image, _, ImageDraw = _require_pillow()

    cells = [_downscaled(data, cell_width) for _, data in pages]
    cell_h = max(c.height for c in cells)
    rows = (len(cells) + cols - 1) // cols
    pad = 12
    label_h = 16

    sheet_w = cols * cell_width + pad * (cols + 1)
    sheet_h = rows * (cell_h + label_h) + pad * (rows + 1)
    sheet = Image.new("RGB", (sheet_w, sheet_h), (28, 28, 30))
    draw = ImageDraw.Draw(sheet)

    for index, cell in enumerate(cells):
        row, col = divmod(index, cols)
        x = pad + col * (cell_width + pad)
        y = pad + row * (cell_h + label_h + pad)
        draw.text((x, y), f"p{index + 1}", fill=(235, 235, 235))
        sheet.paste(cell, (x, y + label_h))
    return sheet


def _out_dir() -> Path:
    OUT_ROOT.mkdir(parents=True, exist_ok=True)
    return OUT_ROOT


def format_measurements(pages: list[tuple[str, bytes]]) -> str:
    """The `--measure` report: one line per page, then the deck-level checks."""
    lines = []
    sizes = set()
    for index, (label, data) in enumerate(pages, start=1):
        m = measure_page(data)
        sizes.add((m["width"], m["height"]))
        if m["blank"]:
            lines.append(f"p{index:<3} {label:<24} {m['width']}x{m['height']}  BLANK")
            continue
        pct = m["margins_pct"]
        lines.append(
            f"p{index:<3} {label:<24} {m['width']}x{m['height']}  "
            f"margins L{pct['left']}% T{pct['top']}% "
            f"R{pct['right']}% B{pct['bottom']}%"
        )

    lines.append("")
    lines.append(f"pages: {len(pages)}")
    if len(sizes) > 1:
        rendered = ", ".join(f"{w}x{h}" for w, h in sorted(sizes))
        lines.append(f"WARNING: mixed page sizes: {rendered}")
    dupes = duplicate_groups(pages)
    if dupes:
        for group in dupes:
            joined = ", ".join(f"p{i}" for i in group)
            lines.append(f"WARNING: identical pages: {joined}")
    else:
        lines.append("distinct pages: all")
    return "\n".join(lines)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="render_review.py")
    parser.add_argument("source", help="an exported .pptx, or a dir of page images")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--measure",
        action="store_true",
        help="numbers only, no image written (the cheapest question)",
    )
    mode.add_argument(
        "--montage",
        action="store_true",
        help="write one labelled contact sheet for the whole deck",
    )
    mode.add_argument(
        "--page", type=int, default=None, help="write one downscaled page (1-based)"
    )
    parser.add_argument(
        "--width",
        type=int,
        default=None,
        help=f"review width (default {DEFAULT_PAGE_WIDTH} for --page, "
        f"{DEFAULT_CELL_WIDTH} per cell for --montage)",
    )
    parser.add_argument(
        "--cols", type=int, default=DEFAULT_COLS, help="montage columns"
    )
    parser.add_argument("--out", default=None, help="output dir (default a temp dir)")
    args = parser.parse_args(argv[1:])

    if args.cols < 1:
        raise RenderReviewError("--cols must be at least 1")

    pages = load_pages(Path(args.source))

    if args.measure:
        print(format_measurements(pages))
        return 0

    out_dir = Path(args.out) if args.out else _out_dir()
    out_dir.mkdir(parents=True, exist_ok=True)
    stem = Path(args.source).stem

    if args.page is not None:
        if not 1 <= args.page <= len(pages):
            raise RenderReviewError(
                f"--page {args.page} is out of range (the deck has {len(pages)})"
            )
        width = args.width or DEFAULT_PAGE_WIDTH
        img = _downscaled(pages[args.page - 1][1], width)
        target = out_dir / f"{stem}-p{args.page}-{img.width}w.png"
        img.save(target)
        print(target)
        print(
            f"render-review | page {args.page}/{len(pages)} at {img.width}x{img.height}",
            file=sys.stderr,
        )
        return 0

    # Montage is the default: it is the mode that answers the recurring
    # question, and defaulting to it is what makes the cheap path the easy one.
    width = args.width or DEFAULT_CELL_WIDTH
    sheet = build_montage(pages, args.cols, width)
    target = out_dir / f"{stem}-contact-sheet.png"
    sheet.save(target)
    print(target)
    print(
        f"render-review | {len(pages)} page(s) in a {args.cols}-wide sheet "
        f"at {sheet.width}x{sheet.height}",
        file=sys.stderr,
    )
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except RenderReviewError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
