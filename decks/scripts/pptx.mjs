// cspell:word OOXML
// cspell:word nodebuffer
// cspell:word letterboxing

import PptxGenJS from "pptxgenjs";

/**
 * A PowerPoint (`.pptx`) writer: one full-bleed picture per slide.
 *
 * This exists because the accelerator merges every team's slides into one
 * Google Slides meta-deck, and Slides' `File ▸ Import slides` accepts only
 * `.pptx` / `.ppt` / an existing Slides deck — it cannot import a PDF at all.
 * So a PDF, however clean, is not a deliverable here; a `.pptx` is.
 *
 * Full-bleed pictures rather than real text boxes is a deliberate trade. The
 * deck's typography, blend modes and layout are CSS the Spectacle renderer
 * already gets right, and no OOXML translation of it would survive contact
 * with Slides' own layout engine. Shipping pixels means what the accelerator
 * projects is exactly what was reviewed. The cost is that the imported slides
 * are not text-editable in Slides — a whole page is replaced, or nothing.
 *
 * The OOXML itself is generated rather than hand-written. An earlier version of
 * this file assembled the package by hand — a presentation, one master, one
 * blank layout, a theme, and the slides, and nothing else — on the reasoning
 * that a picture-per-slide deck needs very little of the format. The output
 * opened in Google Slides and in Keynote, but PowerPoint refused it with
 * "PowerPoint found a problem with content … click Repair", then repaired it by
 * silently dropping content.
 *
 * Two rounds of diagnosis are why this is now a library call. The first bug was
 * a real schema violation — `p:clrMap` emitted on slides and layouts, where the
 * format allows only `p:clrMapOvr` — and fixing it did not stop the prompt. The
 * second was never found: rebuilding the identical pixels and page geometry
 * through a mature writer produced a file PowerPoint opened silently, and a
 * part-by-part comparison showed our *slide* XML was already correct. The fault
 * was somewhere in the scaffolding, where every remaining difference was legal
 * per ECMA-376 but not what PowerPoint writes. That is not a bug the spec can
 * settle, and handing an accelerator a deck that prompts for repair is not
 * worth the dependency saved.
 */

/**
 * Widescreen 16:9 at 10in × 5.625in — **Google Slides' own** widescreen page,
 * not PowerPoint's 13.333in × 7.5in.
 *
 * Both are 16:9, so either avoids letterboxing, and PowerPoint's is the more
 * conventional choice. Slides is the destination that matters here, though, and
 * it resizes an imported deck to the page its own presentation uses. Declaring
 * that page exactly means the slides arrive at 1:1 and are placed rather than
 * rescaled — one fewer resample of an image whose whole content is text and
 * screenshots, which is what resampling damages most.
 *
 * 10 × 5.625 is also exactly 16:9, where 13.333 is a rounding of 40/3 and lands
 * a few EMU off, leaving a sliver of letterboxing that has to go somewhere.
 */
const SLIDE_W_IN = 10;
const SLIDE_H_IN = 5.625;

/** Names the custom page above; the library keys `pptx.layout` off it. */
const LAYOUT_NAME = "GOOGLE_SLIDES_16X9";

/**
 * Build a `.pptx` from one PNG buffer per page, in order, and return it as a
 * Node buffer ready to write.
 */
export async function buildPptx(pages) {
  if (pages.length === 0) throw new Error("refusing to build an empty deck");

  const pptx = new PptxGenJS();
  pptx.defineLayout({ name: LAYOUT_NAME, width: SLIDE_W_IN, height: SLIDE_H_IN });
  pptx.layout = LAYOUT_NAME;

  for (const png of pages) {
    // `Buffer.from` rather than `png.toString`: Puppeteer's screenshot return
    // type has moved between `Buffer` and `Uint8Array` across versions, and
    // `toString("base64")` on a bare `Uint8Array` silently yields a
    // comma-separated list of byte values instead of base64.
    const data = Buffer.from(png).toString("base64");

    pptx.addSlide().addImage({
      data: `image/png;base64,${data}`,
      x: 0,
      y: 0,
      w: SLIDE_W_IN,
      h: SLIDE_H_IN,
    });
  }

  const out = await pptx.write({ outputType: "nodebuffer", compression: true });

  // `write` is typed as the union of every output the library can produce, and
  // the option that narrows it is a runtime value, so the cast is what carries
  // `nodebuffer` into the type. Callers get the `Buffer` this has always
  // returned rather than having to re-narrow it themselves.
  return /** @type {Buffer} */ (out);
}
