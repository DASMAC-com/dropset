import JSZip from "jszip";

/**
 * A minimal PowerPoint (`.pptx`) writer: one full-bleed picture per slide.
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
 * Only the parts of the format that a picture-per-slide deck actually needs
 * are written: a presentation, one master, one blank layout, a theme, and the
 * slides. `docProps` is omitted entirely rather than written empty.
 */

/** EMU per inch — the English Metric Unit is OOXML's base length. */
const EMU_PER_INCH = 914400;

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
const SLIDE_W = Math.round(10 * EMU_PER_INCH);
const SLIDE_H = Math.round(5.625 * EMU_PER_INCH);

const XML_DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>';

const NS_P =
  'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" ' +
  'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" ' +
  'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"';

const contentTypes = (count) =>
  `${XML_DECL}
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="png" ContentType="image/png"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
<Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
<Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
${range(count)
  .map(
    (i) =>
      `<Override PartName="/ppt/slides/slide${i + 1}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>`,
  )
  .join("\n")}
</Types>`;

const rootRels = `${XML_DECL}
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>`;

/**
 * Slide ids start at 256: ids below that are reserved by the format, and
 * PowerPoint rejects a presentation that uses them.
 */
const FIRST_SLIDE_ID = 256;

const presentation = (count) =>
  `${XML_DECL}
<p:presentation ${NS_P}>
<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>
<p:sldIdLst>
${range(count)
  .map(
    (i) =>
      `<p:sldId id="${FIRST_SLIDE_ID + i}" r:id="rId${i + 2}"/>`,
  )
  .join("\n")}
</p:sldIdLst>
<p:sldSz cx="${SLIDE_W}" cy="${SLIDE_H}"/>
<p:notesSz cx="${SLIDE_H}" cy="${SLIDE_W}"/>
</p:presentation>`;

/**
 * The master is `rId1` and the slides run from `rId2`, which is what the
 * `sldMasterIdLst` / `sldIdLst` above assume; the theme takes the id after the
 * last slide.
 */
const presentationRels = (count) =>
  `${XML_DECL}
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
${range(count)
  .map(
    (i) =>
      `<Relationship Id="rId${i + 2}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide${i + 1}.xml"/>`,
  )
  .join("\n")}
<Relationship Id="rId${count + 2}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>
</Relationships>`;

/**
 * The identity colour map every part needs; the deck carries no theme colours
 * of its own because each slide is a single opaque picture.
 */
const CLR_MAP =
  '<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" hlink="hlink" folHlink="folHlink"/>';

const EMPTY_SPTREE = `<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
</p:spTree>`;

const slideMaster = `${XML_DECL}
<p:sldMaster ${NS_P}>
<p:cSld>${EMPTY_SPTREE}</p:cSld>
${CLR_MAP}
<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>
</p:sldMaster>`;

const slideMasterRels = `${XML_DECL}
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>`;

/** `type="blank"` — the picture is the whole page, so there is no placeholder. */
const slideLayout = `${XML_DECL}
<p:sldLayout ${NS_P} type="blank" preserve="1">
<p:cSld name="Blank">${EMPTY_SPTREE}</p:cSld>
${CLR_MAP}
</p:sldLayout>`;

const slideLayoutRels = `${XML_DECL}
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>`;

/**
 * One picture, positioned at the origin at exactly the slide's own size.
 *
 * `<a:stretch><a:fillRect/></a:stretch>` maps the whole image onto the whole
 * frame, so a 16:9 screenshot on a 16:9 page lands 1:1 with no crop and no
 * bars.
 */
const slide = (index) =>
  `${XML_DECL}
<p:sld ${NS_P}>
<p:cSld>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
<p:pic>
<p:nvPicPr>
<p:cNvPr id="2" name="Page ${index + 1}"/>
<p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr>
<p:nvPr/>
</p:nvPicPr>
<p:blipFill><a:blip r:embed="rId1"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>
<p:spPr>
<a:xfrm><a:off x="0" y="0"/><a:ext cx="${SLIDE_W}" cy="${SLIDE_H}"/></a:xfrm>
<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
</p:spPr>
</p:pic>
</p:spTree>
</p:cSld>
${CLR_MAP}
</p:sld>`;

const slideRels = (index) =>
  `${XML_DECL}
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/page${index + 1}.png"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
</Relationships>`;

/**
 * A theme is mandatory — the master's relationships point at one and readers
 * fault without it — but nothing here consumes it, so this is the smallest
 * schema-valid theme rather than a design. The colour scheme's twelve slots,
 * and three entries in each of the four format lists, are the format's floor.
 */
const theme = `${XML_DECL}
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Dropset">
<a:themeElements>
<a:clrScheme name="Dropset">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="000000"/></a:dk2>
<a:lt2><a:srgbClr val="FFFFFF"/></a:lt2>
<a:accent1><a:srgbClr val="4A8CFF"/></a:accent1>
<a:accent2><a:srgbClr val="27E36B"/></a:accent2>
<a:accent3><a:srgbClr val="808080"/></a:accent3>
<a:accent4><a:srgbClr val="808080"/></a:accent4>
<a:accent5><a:srgbClr val="808080"/></a:accent5>
<a:accent6><a:srgbClr val="808080"/></a:accent6>
<a:hlink><a:srgbClr val="4A8CFF"/></a:hlink>
<a:folHlink><a:srgbClr val="808080"/></a:folHlink>
</a:clrScheme>
<a:fontScheme name="Dropset">
<a:majorFont><a:latin typeface="Inter"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>
<a:minorFont><a:latin typeface="Inter"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>
</a:fontScheme>
<a:fmtScheme name="Dropset">
<a:fillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:fillStyleLst>
<a:lnStyleLst>
<a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
</a:lnStyleLst>
<a:effectStyleLst>
<a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst/></a:effectStyle>
</a:effectStyleLst>
<a:bgFillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:bgFillStyleLst>
</a:fmtScheme>
</a:themeElements>
</a:theme>`;

const range = (n) => Array.from({ length: n }, (_, i) => i);

/**
 * Build a `.pptx` from one PNG buffer per page, in order, and return it as a
 * Node buffer ready to write.
 */
export async function buildPptx(pages) {
  if (pages.length === 0) throw new Error("refusing to build an empty deck");

  const zip = new JSZip();

  zip.file("[Content_Types].xml", contentTypes(pages.length));
  zip.file("_rels/.rels", rootRels);
  zip.file("ppt/presentation.xml", presentation(pages.length));
  zip.file("ppt/_rels/presentation.xml.rels", presentationRels(pages.length));
  zip.file("ppt/slideMasters/slideMaster1.xml", slideMaster);
  zip.file("ppt/slideMasters/_rels/slideMaster1.xml.rels", slideMasterRels);
  zip.file("ppt/slideLayouts/slideLayout1.xml", slideLayout);
  zip.file("ppt/slideLayouts/_rels/slideLayout1.xml.rels", slideLayoutRels);
  zip.file("ppt/theme/theme1.xml", theme);

  pages.forEach((png, i) => {
    zip.file(`ppt/slides/slide${i + 1}.xml`, slide(i));
    zip.file(`ppt/slides/_rels/slide${i + 1}.xml.rels`, slideRels(i));
    zip.file(`ppt/media/page${i + 1}.png`, png);
  });

  return zip.generateAsync({
    type: "nodebuffer",
    compression: "DEFLATE",
    compressionOptions: { level: 6 },
  });
}
