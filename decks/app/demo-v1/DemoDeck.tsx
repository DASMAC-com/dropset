"use client";

import {
  Box,
  Deck,
  FlexBox,
  Heading,
  Image,
  Link,
  Notes,
  Progress,
  Slide,
  Text,
} from "spectacle";
import { colors, deckTheme } from "@/theme/tokens";

/**
 * The demo-day pitch deck — a ~2-minute accelerator pitch. Slides are backdrops
 * the presenter talks over; the full spoken script lives in each slide's
 * `<Notes>` (presenter mode, `⌘⇧P` — a bare `p` does nothing), never on the
 * slide itself. The route name is public-facing (`/demo-v1`); internal ticket
 * ids never appear here or in the URL.
 *
 * The copy follows `../../demo-v1-spec.md`, which is the source of truth for
 * it. Edit the spec first, then the deck.
 *
 * Rules from that spec that shape everything below:
 *
 * - **Static images only.** No video and no player: a product beat is an
 *   interface screenshot carrying a claim. Nothing on stage depends on a
 *   network, and every slide prints as a flat page for the accelerator's
 *   combined Google Slides meta-deck.
 * - **Solana is the start, never the ceiling.**
 * - **Dropset is the brand; DASMAC recedes.** The company is the boring DevCo
 *   in the background, and the deck names it exactly once — the footer credit.
 *   Not on the title slide, not in the roadmap's beats, not in a team role.
 * - **No competitor is named or shown, anywhere** — not on a slide and not in
 *   a `Notes` track. A two-minute pitch cannot afford to give competitors free
 *   press, and naming them invites the room to evaluate them instead of us.
 *   Page 10 makes the argument as a property of public liquidity instead. The
 *   fuller answers live in the spec's appendix, for questions only.
 *
 * Twelve pages. Pages 2–3 are the open, split into two beats so the pitch
 * starts with momentum: a huge market, and no penetration of it. Pages 4–7 are
 * one argument in sequence: the swap flow works today, we curate the data for
 * every currency, most of them have no liquidity at all, and the eCLOB is what
 * we're building to fix that. Then how we grow, the roadmap, why FX is next,
 * the team, and a close that replays the title and stays up after the talk.
 */

/**
 * The Dropset wordmark.
 *
 * `brand-assets/dropset-wordmark.png` is **opaque**: its transparent-looking
 * surround is really solid black, so on this deck's near-black backdrop it
 * renders as a subtly different dark rectangle around the mark. Screen
 * blending fixes that without a second asset — screening black over anything
 * returns the backdrop untouched, so the box dissolves while the blue braces,
 * green chevrons and white lettering come through as themselves.
 */
const Wordmark = ({ width }: { width: number }) => (
  <img
    src="/dropset-wordmark.png"
    alt="Dropset"
    width={width}
    style={{ display: "block", mixBlendMode: "screen" }}
  />
);

/**
 * Margin between the footer credit's label and the mark, in slide units.
 *
 * Zero, and there is still a visible gap: "Built by" carries appreciably more
 * advance width than ink — measured off a screenshot, the label's box ran ~16
 * units past the "y" — so the space between the words and the mark comes out of
 * the text metrics rather than out of this margin. Raise it only if the label
 * ever changes to something with tighter metrics.
 */
const CREDIT_GAP = 0;

/**
 * Optical centring of the footer credit: slide units to shift it **right** from
 * the mark-centred position the markup produces on its own.
 *
 * This is the one hand-tuned number in the deck, and it exists because the
 * credit is a lockup of two very unequal parts — small grey "Built by" text
 * against a bright mark — so there is no single geometric centre that also
 * *looks* centred. Both candidates were built and measured off screenshots,
 * against the slide's midline taken from a page eyebrow (block-centred, so its
 * ink centre marks it; the reconstruction from Space Mono's metrics agrees with
 * the measurement to well under a unit):
 *
 * - **0** — the mark lands on the midline exactly (measured +0.5), which puts
 *   the lockup as a whole well left of it. Read as too far left.
 * - **`(label width + CREDIT_GAP) / 2`** — the lockup's own box lands on the
 *   midline instead, which pushes the mark right of it by the same amount. At
 *   the 14-unit gap this started with, that was ~52, and the mark then measured
 *   +63; read as too far right.
 *
 * Note that the two ends move with `CREDIT_GAP`: a wider gap gives the label a
 * longer lever, so box-centring lands further right and reads worse. Which is
 * why the pair settled here — gap at 0 and the shift near box-centring — tuned
 * by eye against the rendered footer. **This is the only value to change** if it
 * ever reads off: larger moves the credit right, smaller moves it left.
 */
const CREDIT_OPTICAL_SHIFT = 45;

/**
 * "Built by" — the label half of the footer credit.
 *
 * Rendered **twice**: visibly to the mark's left, and again `mirrored` — same
 * text, same metrics, `visibility: hidden` — to its right. That hidden copy is
 * what makes the row symmetric about the mark, so plain centring puts the mark
 * on the slide's midline exactly, and nothing has to be re-measured if this text
 * ever changes. `CREDIT_OPTICAL_SHIFT` then walks the pair off that reference
 * point to where the lockup looks centred. `visibility: hidden` keeps the space
 * while staying out of the accessibility tree, so the credit is still read once.
 *
 * The margins carry **explicit `px`**, and that is not incidental. Spectacle's
 * `Text` resolves its margin through styled-system, which looks a *unitless*
 * value up in the theme's `space` scale — so `margin="0"` silently becomes
 * `space[0]`, which is 16 units here rather than zero. A multi-value string like
 * this one fails that lookup and passes through literally, which is the only
 * reason the original worked; writing the units makes it deliberate instead of
 * lucky. The 16-unit phantom margin is what once dropped this label below the
 * mark's baseline.
 */
const CreditLabel = ({ mirrored = false }: { mirrored?: boolean }) => (
  <Text
    color="quaternary"
    fontSize="22px"
    margin={
      mirrored ? `0px 0px 0px ${CREDIT_GAP}px` : `0px ${CREDIT_GAP}px 0px 0px`
    }
    style={mirrored ? { visibility: "hidden" } : undefined}
  >
    Built by
  </Text>
);

/**
 * Persistent footer: wordmark on the left, the DASMAC credit in the middle,
 * progress dots on the right. The DASMAC mark is a transparent PNG, so unlike
 * the Dropset one it needs no blend to sit on the dark backdrop.
 *
 * The credit reads **"Built by DASMAC"**, not "Courtesy of" — authorship, not a
 * loan. It is also the **only** place the deck names the company: DASMAC is the
 * boring DevCo in the background, so the title slide, the roadmap and the team
 * roles all stay clear of it and this one small mark carries it alone.
 */
const template = () => (
  <FlexBox
    justifyContent="space-between"
    alignItems="center"
    position="absolute"
    bottom={0}
    width={1}
    zIndex={1}
  >
    <Box padding="0 1.25em">
      <Wordmark width={210} />
    </Box>
    {/* The mirrored label (see `CreditLabel`) makes this row symmetric about the
        mark, so plain centring puts the **mark** on the slide's midline exactly;
        `CREDIT_OPTICAL_SHIFT` then walks the pair back toward where the lockup
        looks centred. Splitting it that way is the point: the structure carries
        an exact reference point, and one named number carries the judgement, so
        tuning the look can never quietly break the geometry.

        Two dead ends worth not repeating. `space-between` on this flex row was
        the original, and it was never the real problem — the 210-unit wordmark
        against ~197 units of progress dots is ~6 units of drift. And centring
        the mark by taking the *label* out of flow works horizontally but loses
        this row's vertical centring, at which point the theme-scale margin trap
        `CreditLabel` describes drops the label below the mark's baseline. So
        the lockup below is positioned, but **its two children both stay in
        flow**, which is what keeps them on one baseline.

        That positioning depends on something one prop away: the lockup's `top`
        and `left` resolve against **this** `FlexBox`, and only because it
        carries `position="absolute"` above. Drop that and the containing block
        becomes Spectacle's `TemplateWrapper`, which is inset to the whole slide
        — `top: 50%` would then centre the credit vertically **on the slide**
        rather than in the footer. */}
    <div
      style={{
        alignItems: "center",
        display: "flex",
        left: "50%",
        position: "absolute",
        top: "50%",
        transform: `translate(calc(-50% + ${CREDIT_OPTICAL_SHIFT}px), -50%)`,
      }}
    >
      <CreditLabel />
      <img
        src="/dasmac-wordmark.png"
        alt="DASMAC"
        width={110}
        style={{ display: "block" }}
      />
      <CreditLabel mirrored />
    </div>
    <Box padding="0 1.25em">
      <Progress color={colors.accent} size={11} />
    </Box>
  </FlexBox>
);

/**
 * Vertical room reserved on each page: a small inset above the content, and
 * enough below it to clear the footer.
 *
 * The two numbers sum to the reserve the page has always carried, so every
 * height budget below still holds and no page can newly overflow. What changed
 * is the split. It was symmetric, which paired with centring to put each page's
 * content on the slide's own midline — mathematically tidy, and wrong to look
 * at: the footer occupies the bottom of the slide, so a centred block leaves a
 * bare band above it and a visibly larger one below, and every page read as
 * hovering in the middle of a frame it never filled.
 *
 * Anchoring to the top instead spends that asymmetry where it does something.
 * Each page now starts its eyebrow at the same height near the top edge, and
 * the slack collects at the bottom, between the content and the footer, where
 * it reads as breathing room rather than as a gap.
 */
const CONTENT_INSET_TOP = 28;
const FOOTER_RESERVE = 78;

/**
 * Every slide's content column.
 *
 * Between this and Spectacle's own 32px slide padding, a page has ~910 of the
 * 1080 slide units to work in. Worth doing the arithmetic when adding to a
 * page: content that overflows is merely scaled down on screen but **silently
 * clipped in the export**, which is the path to the meta-deck.
 */
const SlideBody = ({
  centered = false,
  children,
}: {
  /**
   * Centre the content instead of anchoring it to the top.
   *
   * For the title page only, and the distinction is what the top anchor is
   * *for*: it exists so that every page's eyebrow lands at the same height, and
   * a page with no eyebrow gains nothing from it. The title is a wordmark and
   * one line, so anchoring it high just hangs it off the top edge above a void
   * that is most of the slide.
   */
  centered?: boolean;
  children: React.ReactNode;
}) => (
  <FlexBox
    height="100%"
    flexDirection="column"
    justifyContent={centered ? "center" : "flex-start"}
    padding={`${CONTENT_INSET_TOP}px 0 ${FOOTER_RESERVE}px 0`}
  >
    {children}
  </FlexBox>
);

/**
 * Takes whatever height is left under a page's header and centres its child in
 * it.
 *
 * Top-anchoring lines every page's eyebrow up, which is what it is for, but it
 * also drops the page's content directly under the sentence and leaves the
 * remainder as one band at the bottom. On a page whose content is a single
 * compact figure that reads as a slab pushed to the top of a half-empty slide.
 *
 * Centring the *content* while the header stays anchored gets both: the kicker
 * and sentence still land where they do on every other page, and the figure
 * sits in the middle of the room it actually has. Only pages with one
 * self-contained block below the sentence want this — a page whose content is
 * a list or a column of its own reads better hanging from the sentence.
 */
const SlideFill = ({ children }: { children: React.ReactNode }) => (
  <div
    style={{
      alignItems: "center",
      display: "flex",
      flex: "1 1 auto",
      flexDirection: "column",
      justifyContent: "center",
      // A flex child's `min-height` is `auto`, so a block taller than the space
      // left would refuse to shrink and push past the footer rather than
      // centring. Zero lets the box resolve to the room it actually has.
      minHeight: 0,
      width: "100%",
    }}
  >
    {children}
  </div>
);

/**
 * Small monospace kicker that labels each content slide. Uppercase, letterspaced
 * Space Mono — the exact treatment the company's own tag carries ("DISTRIBUTED
 * ATOMIC STATE MACHINE ALGORITHMS CORPORATION"), so the deck's kickers and the
 * brand's typography are visibly the same system even now that the company
 * banner itself is off the deck. Sentence case stays with Inter, where it
 * belongs.
 *
 * It sits **close** to the sentence it labels — modest bottom margin, and an
 * explicit `lineHeight`, because `Text` sets none and would otherwise inherit
 * the browser's `normal` (~1.21) and hang leading under a single line of 26px
 * type, widening a gap the margin only appears to control. A kicker should read
 * as a label on the sentence rather than as a line of its own; close, but not
 * so close that the two collide at display size.
 */
const Eyebrow = ({ children }: { children: React.ReactNode }) => (
  <Text
    color="secondary"
    fontFamily="monospace"
    fontSize="26px"
    margin="0 0 18px 0"
    style={{
      letterSpacing: "0.14em",
      lineHeight: 1.1,
      textTransform: "uppercase",
    }}
  >
    {children}
  </Text>
);

/**
 * The sentence a page is built around. Sized down from the theme's `h1` and
 * width-capped so a whole sentence lands in one or two lines at a readable
 * measure rather than spanning the slide.
 *
 * **No terminal period, on any page.** Sentence structure still applies — these
 * are clauses with a subject and a verb, not fragment headlines — but at display
 * size a full stop is a visible mark that earns nothing, because there is no
 * following sentence for it to separate. Multi-sentence copy (venue captions,
 * roadmap bodies, team bios) keeps its punctuation, since there the period is
 * doing its actual job.
 */
const Statement = ({
  children,
  fontSize = "76px",
  maxWidth = "1540px",
  nowrap = false,
}: {
  children: React.ReactNode;
  fontSize?: string;
  maxWidth?: string;
  nowrap?: boolean;
}) => (
  <Heading
    fontSize={fontSize}
    // `0px`, not `0`. Spectacle's `Heading` takes its margin through
    // styled-system, which resolves a **unitless** value against the theme's
    // `space` scale — so this read `margin="0"` for a long time and quietly
    // meant `space[0]`, or 16 units, on every side of every page's headline.
    // That is ~32 units of vertical space per page that no page asked for and
    // no margin in this file appeared to control, and it is the single largest
    // reason the dense pages crowded their top edge. Writing the unit skips the
    // lookup. Same trap as `CreditLabel` and `Portrait` — worth checking any
    // bare `0` handed to a Spectacle component in this file.
    margin="0px"
    maxWidth={maxWidth}
    // `nowrap` is the guarantee, where `maxWidth` is only an estimate. Text
    // metrics are what kept breaking these pages: a heading judged to fit one
    // line took two, the page grew by its own line-height, and the overflow
    // clipped the eyebrow off the top. On a page with no height to spare, say
    // "this cannot wrap" and let it be checked at render rather than guessed at
    // authoring time.
    style={nowrap ? { whiteSpace: "nowrap" } : undefined}
  >
    {children}
  </Heading>
);

/**
 * How much of the world's money actually trades on Solana, as a progress bar.
 *
 * A **meter**, not a pie of two slices: the data is a single ratio against a
 * limit, and the empty part of the track is the whole message. The track is a
 * darker step of the fill's own blue ramp (see `colors.meterTrack`) so the bar
 * reads as one scale rather than two categories.
 *
 * It is labeled with the **percentage only**. An earlier version printed
 * "14 on Solana / 162 in the world" at either end of the track, which simply
 * restated the screenshot mounted directly beneath it — and that screenshot is
 * the better place for the raw count, because it's our own page carrying the
 * number, with the URL, so the figure is checkable rather than asserted.
 */
const LISTED_CURRENCIES = 14;
const TOTAL_CURRENCIES = 162;
/**
 * 900, up from the 760 this carried through v2 — and **capped by the
 * screenshot, not by the page**.
 *
 * 760 was sized for a page this block shared: the v2 gap page ran a column of
 * six facts down the left and the meter plus its citation down the right, so
 * it got a little under half the slide. Splitting the gap page in two gave
 * page 3 the whole width, and at 760 the block sat as a small island in a
 * mostly empty slide — sparse in the wrong way, with the screenshot's own
 * figures too small to read from the back of a room, which defeats the point
 * of citing them.
 *
 * The obvious fix — take the full measure, ~1180 — **does not work**, and the
 * reason is worth keeping. `currencies-listed.png` is **876 px wide**, the
 * smallest capture in the deck by some way (the others run 820–1500). The
 * `Screenshot` frame renders its image at `width - 2 * SCREENSHOT_INSET`, so
 * at 1180 the image is asked for 1142 units against 876 native — a 1.3×
 * upscale, and it visibly blurs in the export while every other capture on
 * neighboring pages stays crisp. At 900 the image lands at 862, just inside
 * native, so it is still being *down*scaled and stays sharp.
 *
 * **So this cannot grow without a new capture.** Re-shoot
 * `currencies-listed.png` at a higher device pixel ratio first; until then 900
 * is the ceiling, and the page's remaining slack is deliberate white space
 * rather than room this block should take.
 */
const METER_WIDTH = 900;
const LISTED_SHARE = (LISTED_CURRENCIES / TOTAL_CURRENCIES) * 100;

const CurrencyMeter = () => (
  // Inset to match the screenshot mounted under it: that capture sits inside a
  // bordered, padded frame, so its image starts `SCREENSHOT_INSET` in from the
  // frame's outer edge. Without the same inset the bar and the figure it cites
  // are a padding-width out of true with each other.
  <Box width={`${METER_WIDTH}px`} margin={`0 ${SCREENSHOT_INSET}px`}>
    <FlexBox justifyContent="space-between" alignItems="flex-end">
      <div style={{ color: colors.mutedFg, fontSize: "30px" }}>
        Currencies available on Solana
      </div>
      <div
        style={{
          color: colors.accent,
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "36px",
        }}
      >
        {LISTED_SHARE.toFixed(1)}%
      </div>
    </FlexBox>
    <div
      style={{
        backgroundColor: colors.meterTrack,
        borderRadius: "16px",
        height: "36px",
        marginTop: "16px",
        overflow: "hidden",
        width: "100%",
      }}
    >
      <div
        style={{
          backgroundColor: colors.accent,
          // Rounded only on the free end: the filled end is anchored to the
          // track's own start, and rounding both ends detaches it from the
          // baseline it's measured from.
          borderRadius: "0 16px 16px 0",
          height: "100%",
          width: `${LISTED_SHARE}%`,
        }}
      />
    </div>
  </Box>
);

/**
 * One very large number over a small label — the entirety of page 2's content.
 *
 * The figure is the visual, which is the page's whole design: the first content
 * page of a two-minute pitch has about seven seconds, and a number this size is
 * read in one. It is set in the mono face rather than Inter because the deck
 * already treats mono as its data voice (the meter's percentage, the roadmap's
 * `Now / Next / Later`, every capture caption), so a figure at display size
 * reads as a measurement rather than as a headline that happens to be numeric.
 *
 * Accent-colored, and that is the whole reason page 2 needs no other emphasis:
 * the deck's accent has appeared only on the wordmark's chevrons up to here, so
 * this is the first time the audience sees it carry meaning.
 *
 * **Keep the caption to two or three words.** It labels the figure; it does not
 * qualify it. A caption that grows into a sentence is the first sign this page
 * is turning back into a market-opportunity slide, which the format rules
 * forbid — no breakdown, no CAGR, no TAM ring.
 */
const HeroFigure = ({
  figure,
  caption,
}: {
  figure: string;
  caption: string;
}) => (
  <Box>
    <div
      style={{
        color: colors.accent,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "300px",
        // Explicit, because `normal` would hang ~60 units of leading around a
        // cap this size and the page's whole layout is this one block.
        lineHeight: 1,
        textAlign: "center",
      }}
    >
      {figure}
    </div>
    <div
      style={{
        color: colors.mutedFg,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "34px",
        letterSpacing: "0.14em",
        lineHeight: 1.2,
        marginTop: "26px",
        textAlign: "center",
        textTransform: "uppercase",
      }}
    >
      {caption}
    </div>
  </Box>
);

/**
 * A screenshot frame's chrome, named because anything stacked above or below a
 * capture has to inset by the same amount to line up with the image rather than
 * with the frame's outer edge — see `SCREENSHOT_INSET` and page 3's meter.
 */
const SCREENSHOT_BORDER = 1;
const SCREENSHOT_PAD_X = 18;
const SCREENSHOT_INSET = SCREENSHOT_PAD_X + SCREENSHOT_BORDER;

/**
 * Frames a screen capture so it reads as a window rather than floating art,
 * with an optional caption underneath — `source` for a capture the audience
 * can go look at themselves, `caption` for what to notice in it.
 */
const Screenshot = ({
  src,
  width,
  alt,
  source,
  caption,
  margin = "34px 0 0 0",
}: {
  src: string;
  width: number;
  alt: string;
  source?: string;
  caption?: string;
  margin?: string;
}) => (
  <Box margin={margin}>
    <Box
      border={`${SCREENSHOT_BORDER}px solid ${colors.border}`}
      borderRadius="12px"
      padding={`14px ${SCREENSHOT_PAD_X}px`}
      backgroundColor={colors.muted}
    >
      {/* `display: block` matters: Spectacle's `Image` is a bare styled `img`,
          so it is inline by default and sits on a text baseline — which leaves
          a few units of descender space under every capture and makes each
          frame slightly taller than the image inside it. On pages fighting for
          height that error compounds across three captures. */}
      <Image src={src} width={width} alt={alt} style={{ display: "block" }} />
    </Box>
    {source ? (
      <Link href={`https://${source}`} fontSize="24px" margin="10px 0 0 0">
        {source}
      </Link>
    ) : null}
    {/* Space Mono at the same size the swap-flow labels use, so a capture's
        label reads identically wherever it appears — the only difference is
        that these sit under their image and the swap flow's sit above (that
        page's captures climb in height, so above is what keeps its labels on a
        shared baseline). */}
    {caption ? (
      <div
        style={{
          color: colors.mutedFg,
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "28px",
          lineHeight: 1.3,
          marginTop: "12px",
        }}
      >
        {caption}
      </div>
    ) : null}
  </Box>
);

// The chevron that means "and then". Space Mono, one mark and one face, and
// it is deliberately shared: page 4's swap flow uses it between steps, and
// page 10's flip reuses it between asset classes precisely because page 4 has
// already taught the audience what it means. It used to be the gap page's
// `Fact` bullet marker too; that list is gone, and the glyph now only ever
// marks a sequence.
const SequenceArrow = () => (
  <Box margin="0 16px">
    <div
      style={{
        color: colors.accent,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "44px",
        lineHeight: 1,
      }}
    >
      ›
    </div>
  </Box>
);

/**
 * Column width for a swap-flow step.
 *
 * This is the number that keeps page 4 inside its slide, so it is a named
 * constant rather than repeated at three call sites. The third capture is very
 * tall (820×1371), so it sets the row's height, and the row's height is most of
 * the page: at 410 — with a two-line statement and the URL below the row — the
 * page stacked to ~1008 units against the ~910 a slide has, and flex centring
 * pushed the eyebrow off the top edge (cropped on screen, silently clipped in
 * print).
 *
 * Widening it back to 420 was only safe because two other things now hold: the
 * statement is pinned to one line by `nowrap` (the guarantee — `maxWidth` is
 * only an estimate), and the URL moved into the middle column. Those are
 * load-bearing — undo either and this has to come down further.
 *
 * It came back to 392 once the deck stopped centring its content. The captures
 * are the page's height: the third is by far the tallest, so the row's bottom
 * edge is set by it alone, and at 420 it ran to within ~18 units of the footer.
 * Trimming the width is the only lever that buys clearance without touching the
 * layout — every other number on this page is already at its floor.
 */
const STEP_WIDTH = 392;

/**
 * Horizontal padding inside a step's frame. Named because the image width is
 * `width - 2 * this` — with the number inlined at both sites, changing the
 * padding overflowed the image out of its own frame with nothing to catch it.
 */
const STEP_FRAME_PAD = 14;

/**
 * A numbered step in the swap flow, caption **above** its capture.
 *
 * Above, not below, because the three captures are wildly different heights —
 * a short picker crop, a taller search panel, and a very tall settled trade.
 * Captions underneath landed on three different baselines and read as ragged;
 * above, they align on one line and the images hang from it, which reads as
 * deliberate.
 */
const Step = ({
  step,
  label,
  src,
  width,
  alt,
}: {
  step: number;
  label: string;
  src: string;
  width: number;
  alt: string;
}) => (
  <Box width={`${width}px`}>
    {/* Kept short enough to stay on one line at this size — the labels are
        deliberately terse ("Open the picker", not "Open the currency picker")
        so they can be read from the back of a room without wrapping and
        costing the row height the captures need. */}
    <div
      style={{
        color: colors.mutedFg,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "28px",
        lineHeight: 1.3,
        marginBottom: "16px",
      }}
    >
      <span style={{ color: colors.accent }}>{step}. </span>
      {label}
    </div>
    <Box
      border={`1px solid ${colors.border}`}
      borderRadius="12px"
      padding={`12px ${STEP_FRAME_PAD}px`}
      backgroundColor={colors.muted}
    >
      <Image
        src={src}
        width={width - 2 * STEP_FRAME_PAD}
        alt={alt}
        style={{ display: "block" }}
      />
    </Box>
  </Box>
);

/**
 * A row of captioned logos — the visual on the pages that are about other
 * companies.
 *
 * The marks are each company's own, so they come in two shapes: square icons
 * and logotypes several times as wide as they are tall. Tiles are therefore
 * **height-matched with the width left free** — forcing every mark into one
 * square is what squashed the wide ones flat. Every source here is either
 * transparent or already dark-backed, so tiles carry no fill of their own and
 * the marks sit straight on the deck's black.
 */
type Logo = { name: string; src: string; note?: string };

/**
 * A tile's column is wider than the tile by this much, so a long caption sits
 * on one line; each column also carries `LOGO_GUTTER` on both sides. The
 * flywheel's group rule spans exactly two columns, so it derives its width from
 * these — hence named constants rather than numbers in two places, where
 * editing the caption room here would silently stop the rule there from
 * matching its own pair of tiles.
 */
const LOGO_CAPTION_ROOM = 78;
const LOGO_GUTTER = 14;

const LogoRow = ({
  logos,
  width = 150,
  height = 54,
  tint,
  margin = "34px 0 0 0",
}: {
  logos: Logo[];
  width?: number;
  height?: number;
  tint?: string;
  margin?: string;
}) => (
  <FlexBox margin={margin} justifyContent="center" alignItems="flex-start">
    {logos.map(({ name, src, note }) => (
      // The column is wider than its tile so a long caption ("AUDD Digital")
      // sits on one line: a wrapped caption pushes its note down and breaks
      // the row's shared baseline.
      <Box
        key={name}
        margin={`0 ${LOGO_GUTTER}px`}
        width={`${width + (note ? LOGO_CAPTION_ROOM : 0)}px`}
      >
        {/* Every tile in a row is the same box; the mark is contained inside it
            rather than sized to it, so a wide logotype and a square icon share a
            footprint without either being stretched. */}
        <div
          style={{
            alignItems: "center",
            border: `1px solid ${tint ?? colors.border}`,
            borderRadius: "14px",
            boxSizing: "border-box",
            display: "flex",
            height: `${height + 46}px`,
            justifyContent: "center",
            padding: "0 18px",
            width: `${width}px`,
          }}
        >
          <img
            src={src}
            alt={name}
            style={{
              display: "block",
              maxHeight: `${height}px`,
              maxWidth: "100%",
              objectFit: "contain",
            }}
          />
        </div>
        {/* Captions are plain elements, not `Text`: Spectacle's carries a theme
            margin of its own that opens a gap between a name and its note far
            larger than either margin asks for, and on the flywheel that pushed
            the last line into the footer.

            Names are **always shown**, because every mark left in the deck is
            an icon rather than a wordmark, and an icon says nothing on its own.
            There used to be a `showNames` opt-out for the open-venue page's
            competitor row, where each mark was a logotype and a caption under
            it printed the word twice; that page and those marks are gone, so
            the option went with them rather than sitting here as a switch with
            no caller. The rule it encoded still holds if a wordmark ever
            returns: name an icon, don't name a wordmark. */}
        <div
          style={{
            color: colors.foreground,
            fontFamily: deckTheme.fonts.monospace,
            fontSize: "22px",
            lineHeight: 1.2,
            marginTop: "14px",
          }}
        >
          {name}
        </div>
        {note ? (
          <div
            style={{
              color: colors.mutedFg,
              fontSize: "19px",
              lineHeight: 1.25,
              marginTop: "6px",
            }}
          >
            {note}
          </div>
        ) : null}
      </Box>
    ))}
  </FlexBox>
);

/**
 * The two ends of the flywheel. Issuers sit **upstream**: they mint the
 * currency and need it to trade. Payments companies sit **downstream**: they
 * consume the liquidity to settle real invoices. Naming both directions is the
 * point of the page — a venue needs each end to bootstrap, and these are all
 * companies we've actually spoken with about sourcing liquidity, which is what
 * makes the page customer development rather than a wish list.
 */
const UPSTREAM: Logo[] = [
  { name: "AUDD Digital", src: "/remote/logo-audd.png", note: "AUDD issuer" },
  { name: "Loon", src: "/remote/logo-cadc.png", note: "CADC issuer" },
];

const DOWNSTREAM: Logo[] = [
  { name: "Altitude", src: "/remote/logo-altitude.png", note: "Banking" },
  { name: "CargoBill", src: "/remote/logo-cargobill.png", note: "Supply chain" },
];

// Square: every mark on the flywheel is a pure icon, so `TILE` matches the
// tile's own height (its padding plus the image cap passed below).
const FLYWHEEL_TILE = 104;
// A group is two tile columns, so its heading rule spans exactly its own pair.
// Derived from `LogoRow`'s constants rather than repeating their values.
//
// This adds `LOGO_CAPTION_ROOM` unconditionally, where `LogoRow` adds it only
// to a column that carries a `note`. The two agree because every flywheel logo
// has one — give an upstream or downstream entry no note and the heading rule
// silently stops spanning its own pair, which is the misalignment that room
// exists to prevent.
const FLYWHEEL_GROUP =
  2 * (FLYWHEEL_TILE + LOGO_CAPTION_ROOM + 2 * LOGO_GUTTER);
const FLYWHEEL_WIDTH = 2 * FLYWHEEL_GROUP + 80;

/**
 * One end of the flywheel: a heading with a rule under it, spanning both of
 * that end's tiles. Boxing each end in a filled panel did make the split
 * obvious, but at the cost of two slabs that dominated the page — the
 * underline says "these two belong together" with none of that weight.
 */
const FlywheelEnd = ({ label, logos }: { label: string; logos: Logo[] }) => (
  <Box width={`${FLYWHEEL_GROUP}px`}>
    <div
      style={{
        color: colors.accent,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "24px",
        letterSpacing: "0.12em",
        lineHeight: 1.2,
        marginBottom: "10px",
        textTransform: "uppercase",
      }}
    >
      {label}
    </div>
    <div
      style={{ backgroundColor: colors.accent, height: "2px", width: "100%" }}
    />
    <LogoRow
      logos={logos}
      width={FLYWHEEL_TILE}
      height={58}
      margin="20px 0 0 0"
    />
  </Box>
);

/**
 * The flywheel, as its two ends. The curve behind them is depth growing once
 * both ends are connected — an inline SVG, so it stays crisp at projector size
 * with no asset pipeline.
 *
 * Getting this page to read took several tries, so don't undo it: four evenly
 * spaced tiles look like one row of four, and a hairline between them doesn't
 * change that. Heading-plus-rule is what brackets a pair without the weight of
 * a filled panel. The curve has to be wide, too — a narrow one above a vertical
 * divider read as a chart mounted on a stick, which is why there's no divider.
 */
const Flywheel = () => (
  <Box margin="22px 0 0 0" width={`${FLYWHEEL_WIDTH}px`}>
    <FlexBox justifyContent="center" margin="0 0 24px 0">
      <svg
        width="860"
        height="170"
        viewBox="0 0 860 170"
        role="img"
        aria-label="Order-book depth growing over time"
      >
        <path
          d="M0 162 C 274 158, 469 136, 613 87 C 730 47, 795 21, 860 5 L 860 170 L 0 170 Z"
          fill={colors.accent}
          opacity="0.18"
        />
        <path
          d="M0 162 C 274 158, 469 136, 613 87 C 730 47, 795 21, 860 5"
          fill="none"
          stroke={colors.accent}
          strokeWidth="6"
        />
      </svg>
    </FlexBox>
    <FlexBox justifyContent="space-between" alignItems="flex-start">
      <FlywheelEnd label="Upstream" logos={UPSTREAM} />
      <FlywheelEnd label="Downstream" logos={DOWNSTREAM} />
    </FlexBox>
  </Box>
);

/**
 * The growth roadmap: three beats in time order along a rule, spanning the page.
 *
 * A **roadmap, not a list** — the distinction is the page. Three bullets read as
 * three guesses about revenue; three beats on a timeline read as a deliberate
 * path. The dot sits on the rule so the eye takes the order before it reads any
 * of the copy.
 */
/**
 * A beat is a label and an action, and **no body**.
 *
 * Each used to carry a two-line paragraph under its headline, and cutting them
 * was the sign-off review's single most concrete note. Three headlines plus
 * three paragraphs is six things to read on a page whose job is to be taken in
 * at a glance — and the paragraphs were exactly what the presenter was *saying*
 * at that moment, so printing them had the audience reading ahead of the talk.
 * They moved into this slide's `Notes` intact; nothing was lost, and the page
 * now reads in about a second.
 */
type Beat = { when: string; headline: string };

const BEATS: Beat[] = [
  // Imperatives, not "We onboard…": the `when` label above each headline
  // already supplies the subject and the tense, so a pronoun in each one is
  // three words of scaffolding restating what the rule beneath them says. This
  // page is the deck's one sanctioned place for the imperative mood — every
  // other line is a full sentence.
  //
  // Plain-spoken customer development, and it **names no company**: an earlier
  // draft read "DASMAC bootstraps liquidity", which put the DevCo back on a
  // slide it has receded from and made the first beat about us rather than
  // about the issuers. Both sides of the two-sided market are in the talk
  // track — the issuers upstream and the pipeline of companies that consume the
  // liquidity downstream (the page-8 names) — because seeding one side is a
  // liquidity operation, and seeding both is a market.
  { when: "Now", headline: "Onboard emerging stablecoin issuers" },
  // Two words on the slide, so the load-bearing half of this beat is now
  // **entirely spoken**: that *other* market makers enter is what says this
  // isn't a prop AMM — anyone can quote here, so quotes compete tighter rather
  // than being set by whoever owns the venue, and that competition is the
  // mechanism the compounding runs on. Fees only mean something after it. If
  // the talk track is ever trimmed, protect that sentence: "Accrue protocol
  // fees" standing alone is the one line on this page that can be misread as a
  // rent-extraction plan.
  { when: "Next", headline: "Accrue protocol fees" },
  // Left broad on purpose — "beyond spot" lets a reader fill in their own
  // derivatives thesis, where naming one hands them ours to argue with. The
  // business cases (treasury management, hedging B2B payment flows) are in the
  // talk track, as examples rather than a list: the beat is that derivatives
  // buy both a better market structure and downstream uses of it.
  { when: "Later", headline: "Expand beyond spot" },
];

/**
 * Timeline geometry. The full content measure, so the roadmap spans the page
 * rather than sitting as a narrow band in the middle of it.
 *
 * **Pitch is derived, and that is the point.** The dots used to be a flex row of
 * three equal-width segments while the text below was `space-between` on a fixed
 * column width — two different geometries, so they only agreed on the first
 * column. The second dot sat 27 units left of its heading and the third 53,
 * which read as sloppy rather than as a mistake. Both rows now come from the
 * same pitch, so a dot cannot drift from the word beneath it: change
 * `ROADMAP_COLUMN` and they move together.
 *
 * The derivation assumes **`BEATS.length * ROADMAP_COLUMN <= ROADMAP_WIDTH`**
 * with at least two beats. A fourth beat at the current column width makes the
 * gap negative — flex would shrink the columns to fit while the absolutely
 * placed dots would not, silently reintroducing the very drift this replaced —
 * so widen `ROADMAP_WIDTH` or narrow `ROADMAP_COLUMN` before adding one.
 */
const ROADMAP_WIDTH = 1780;
const ROADMAP_COLUMN = 540;
const ROADMAP_DOT = 18;
const ROADMAP_GAP =
  (ROADMAP_WIDTH - BEATS.length * ROADMAP_COLUMN) / (BEATS.length - 1);
const ROADMAP_PITCH = ROADMAP_COLUMN + ROADMAP_GAP;

const Roadmap = () => (
  <Box margin="46px 0 0 0" width={`${ROADMAP_WIDTH}px`}>
    {/* One unbroken rule with the dots absolutely placed on it, rather than a
        rule segment per column: a per-column border restarts at every gap, and
        the continuous line is what makes the three beats read as one sequence.
        Absolute placement is also what pins each dot to its own column's left
        edge. */}
    <div style={{ height: `${ROADMAP_DOT}px`, position: "relative" }}>
      {/* Runs centre-of-first-dot to centre-of-last, so it never overshoots
          into empty space at either end. */}
      <div
        style={{
          backgroundColor: colors.border,
          height: "2px",
          left: `${ROADMAP_DOT / 2}px`,
          position: "absolute",
          top: `${ROADMAP_DOT / 2 - 1}px`,
          width: `${(BEATS.length - 1) * ROADMAP_PITCH}px`,
        }}
      />
      {BEATS.map(({ when }, index) => (
        <div
          key={when}
          style={{
            backgroundColor: colors.accent,
            borderRadius: "50%",
            height: `${ROADMAP_DOT}px`,
            left: `${index * ROADMAP_PITCH}px`,
            position: "absolute",
            top: 0,
            width: `${ROADMAP_DOT}px`,
          }}
        />
      ))}
    </div>
    <FlexBox justifyContent="space-between" alignItems="flex-start">
      {BEATS.map(({ when, headline }) => (
        <Box key={when} width={`${ROADMAP_COLUMN}px`} margin="26px 0 0 0">
          <div
            style={{
              color: colors.accent,
              fontFamily: deckTheme.fonts.monospace,
              fontSize: "24px",
              letterSpacing: "0.12em",
              lineHeight: 1.2,
              textTransform: "uppercase",
            }}
          >
            {when}
          </div>
          {/* 42px, up from the 34 this carried while a body paragraph sat
              under it. The headline is now the column's only content, and at
              the old size three short action phrases read as captions floating
              on a mostly empty page rather than as the page's substance —
              cutting the bodies freed ~120 units per column, and giving a
              third of it back to the type is what keeps the page from looking
              like something went missing. */}
          <div
            style={{
              color: colors.foreground,
              fontSize: "42px",
              lineHeight: 1.25,
              marginTop: "16px",
            }}
          >
            {headline}
          </div>
        </Box>
      ))}
    </FlexBox>
  </Box>
);

/**
 * The page-10 flip: three asset classes in the order they came onchain, the
 * third being ours.
 *
 * This **replaces** v2's two-panel open-venue comparison — three
 * permissioned-rail logos red-outlined against the Dropset wordmark in green,
 * with three and four bullets under them. The sign-off review's objection was
 * structural: in a two-minute pitch that is a long wind-up to a contrast, and
 * it spends its seconds defining the other side's position before arguing with
 * it. The flip makes the same point forward — the pattern already happened
 * twice, so the third is the audience's own inference — at three words a beat
 * instead of seven bullets.
 *
 * The order is an **escalation in seriousness**, not chronology for its own
 * sake, and it has to stay this way. Opening on memecoins is deliberate and
 * slightly risky: it is the beat an investor might read as unserious, which is
 * exactly why the presenter names it plainly and moves on. Ducking it would
 * cost the page its first data point and its credibility — a deck that says
 * "token launches" when everyone knows it means memecoins is being evasive
 * about something it has no need to be evasive about. Tokenized equities is
 * the **pivot**: a real, regulated, trillion-dollar asset class that went
 * almost entirely to one chain, and the beat that earns FX.
 *
 * **Three, never four.** RWAs are the obvious fourth and they are deliberately
 * spoken instead — Solana is third by total RWA value behind Ethereum and BNB,
 * so the honest claim is about *momentum* (fastest growth, most holders,
 * highest 30-day inflows) and that needs a sentence, not a tile.
 *
 * Every figure here is checkable, which is the standard this page is held to.
 * Verified 2026-08-11, and **re-verify before presenting**:
 *
 * - **96%** — Solana took >96% of onchain tokenized-equity volume in June 2026
 *   ($3.47B that month, $5.77B across Q2, a quarterly ATH). Reported at 95–97%
 *   depending on the window; 96 is the conservative round number.
 * - **11.9M** — cumulative token launches on Solana's dominant launchpad since
 *   January 2024. Deliberately a *one-platform* figure, so it understates the
 *   chain total: it is a floor, and a floor is the defensible direction.
 * - **$9T+** — the same figure page 2 opens on, deliberately repeated. It is
 *   what makes the third tile land as a bigger prize than the two before it
 *   rather than as a promise, and it closes the loop the open started.
 *
 * The review's suggested "99% of tokens are created on Solana" framing is
 * **cut**: no source supports it, and the weaker "more new tokens than every
 * other chain combined" could not be verified against a current dashboard
 * either. An unverifiable number beside two verified ones is what makes an
 * audience doubt all three.
 */
type FlipBeat = { label: string; value: string; unit: string; claim?: boolean };

const FLIP_BEATS: FlipBeat[] = [
  // **"12M+", not "11.9M", and the `+` is the whole point.** The precise
  // figure is one launchpad's 11.9M cumulative launches since January 2024 —
  // but that launchpad is only ~71% of Solana's daily token creation, and
  // other launchpads exist, so the chain's real total is *above* it. Printing
  // "11.9M" would state a one-platform number as though it were the chain's,
  // which is precise and wrong; "over 12M" states a **floor**, which is
  // imprecise and true. A skeptic can only discover the real figure is
  // larger, which is the safe direction to be wrong in.
  //
  // What is deliberately NOT here is a cross-chain comparison ("more token
  // launches than any other chain"). It is widely repeated and probably true,
  // and it could not be sourced to a current, citable dashboard — so it stays
  // off the slide *and* out of the talk track.
  { label: "Memecoins", value: "12M+", unit: "tokens launched on Solana" },
  // "on Solana" is load-bearing here too: 96% of onchain tokenized-equity
  // volume being *on Solana* is the claim, and the page never names Solana
  // anywhere else — not in the headline, not in the accent line — so without
  // it this tile reads as "96% of some unspecified whole".
  {
    label: "Tokenized equities",
    value: "96%",
    unit: "of onchain volume on Solana",
  },
  {
    label: "Foreign exchange",
    value: "Next",
    unit: "$9T+ a day",
    claim: true,
  },
];

const FLIP_TILE_WIDTH = 500;

/**
 * One beat of the flip: a label, a figure, and its unit, in a bordered tile.
 *
 * `claim` tints the whole tile with the deck accent, and exactly one beat gets
 * it. The first two are things that already happened and are therefore
 * evidence; the third is the argument, and it is the only element on the page
 * that isn't a fact about the world as it is. Tinting the tile rather than just
 * its text is what makes the row read left-to-right as *arriving* somewhere —
 * two in the deck's neutral border, then one lit up.
 *
 * The three-part split (label / value / unit) exists so the tiles stay the same
 * height without any of them being padded to match. Written as one string per
 * tile, the two data beats ran long enough to wrap to two lines while "Next"
 * took one, and the row went ragged; split, the big line is always one
 * word-ish and the small line always one line. That headroom is what let the
 * units later grow to carry "on Solana" without the row going uneven again.
 */
const FlipTile = ({ label, value, unit, claim }: FlipBeat) => (
  <Box
    width={`${FLIP_TILE_WIDTH}px`}
    border={`2px solid ${claim ? colors.accent : colors.border}`}
    borderRadius="16px"
    padding="40px 30px"
  >
    <div
      style={{
        color: claim ? colors.accent : colors.mutedFg,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "24px",
        letterSpacing: "0.12em",
        lineHeight: 1.2,
        textTransform: "uppercase",
      }}
    >
      {label}
    </div>
    <div
      style={{
        color: claim ? colors.accent : colors.foreground,
        fontSize: "64px",
        lineHeight: 1.2,
        marginTop: "16px",
      }}
    >
      {value}
    </div>
    <div
      style={{ color: colors.mutedFg, fontSize: "26px", lineHeight: 1.3 }}
    >
      {unit}
    </div>
  </Box>
);

/**
 * The flip row: the three beats with the deck's chevron between them.
 *
 * The chevron is `SequenceArrow`, the same mark page 4's swap flow uses, and
 * reusing it is the point — that page already taught the audience that this
 * glyph means "and then". It is also what keeps the row from reading as a
 * bullet list: global rule 8 allows no lists at all now, and these are peers in
 * a sequence rather than enumerated items.
 */
const Flip = () => (
  <FlexBox justifyContent="center" alignItems="stretch">
    {FLIP_BEATS.map((beat, index) => (
      <FlexBox key={beat.label} alignItems="center">
        {index > 0 ? <SequenceArrow /> : null}
        <FlipTile {...beat} />
      </FlexBox>
    ))}
  </FlexBox>
);

/**
 * Team headshots, mirrored from the marketing site at build time, captioned.
 * Left square and unframed — the sources are already square, and both photos
 * are shot on a dark background that reads as part of the slide.
 *
 * The last page stays on screen after the talk ends, so it can carry more than
 * a line each — but the bios state **what each person has done** and stop there.
 * An earlier draft argued why each role mattered, which read as defending the
 * team rather than describing it.
 */
const Portrait = ({
  src,
  name,
  role,
  prior,
  bio,
}: {
  src: string;
  name: string;
  role: string;
  prior: string;
  bio: string;
}) => (
  <Box margin="0 40px" width="620px">
    <Image src={src} width={180} height={180} alt={name} />
    {/* Name, role and prior run with **no margin between them at all** — their
        `lineHeight` is the only separation, which is why each carries one
        explicitly. Spectacle's `Text` sets no line-height, so it inherits the
        browser's `normal` (~1.21 for Inter), and at 38px that hangs ~19 units of
        leading around a ~27-unit cap height. Stacked three deep, the leading was
        most of the spacing, so trimming margins alone barely moved it and going
        below zero was not an option. Tightening the leading is what actually
        closed these up; the margins that remain (headshot to name, and the rule
        of white space before the bio) are the only two gaps meant to read as
        gaps.

        Note the `0px`, not `0`: a unitless margin gets resolved against the
        theme's `space` scale, so `margin="0"` would silently mean 16 units — the
        same trap `CreditLabel` documents, and here it would add the exact space
        this is removing.

        The bio deliberately keeps the inherited leading: it is the one
        multi-line block, and 1.21 is already tight for five lines of prose. */}
    <Text fontSize="38px" margin="12px 0 0 0" style={{ lineHeight: 1.05 }}>
      {name}
    </Text>
    <Text fontSize="28px" margin="0px" style={{ lineHeight: 1.1 }}>
      {role}
    </Text>
    <Text
      color="secondary"
      fontFamily="monospace"
      fontSize="23px"
      margin="0px"
      style={{ lineHeight: 1.2 }}
    >
      {prior}
    </Text>
    <Text color="quaternary" fontSize="26px" margin="12px 0 0 0">
      {bio}
    </Text>
  </Box>
);

/**
 * The tagline — the deck's one-liner, and the only line on pages 1 and 12.
 *
 * It replaced "Where currency trades onchain" (accurate, and it asked the
 * audience to care on its own), and then "The 24/7/365 currency translation
 * layer", which wrapped badly enough on the slide to read as "The Currency
 * Translation Layer 24…". This one says the actual business: Dropset is a
 * **liquidity** platform, and the promise is that every currency is liquid,
 * for everyone, always — words that land on someone who has never traded FX,
 * which is the job of the one line an investor might read before scrolling
 * past.
 *
 * **"national" is load-bearing.** To a crypto-native audience "every
 * currency" reads as every *token* — "you're making my token liquid?" — and
 * the one word that disambiguates is worth the extra width. The always-on
 * claim moved out of the line and into the page-1 and page-12 spoken openers,
 * which is what keeps page 2's 24/5 fact a payoff rather than an orphan.
 *
 * A noun phrase, and one of global rule 1's three sanctioned fragments: a
 * tagline names the company, and every sentence form ("Dropset is the…") puts
 * a subject on a page whose subject is the wordmark directly above it.
 */
const TAGLINE = "The liquidity layer for every national currency";

/**
 * The title page's body: the wordmark over the tagline, centred, nothing else.
 *
 * **Rendered twice** — page 1 opens on it and page 12 closes on it — which is
 * the whole reason it is a component. The close only works if the page is
 * *identical* to the opener, and two hand-built copies drift the moment either
 * is edited; sharing the body makes that structural rather than a promise.
 * Only the `Notes` differ, which is the point: the same picture carries the
 * one-liner at the start and the why-me at the end.
 *
 * The page used to carry a "Built by DASMAC" line and the wide company banner
 * beneath it; both are gone. DASMAC is the boring DevCo in the background —
 * someone finds it when they sign a document — so a title slide that argues
 * for it argues for the wrong thing. The footer credit is the deck's one
 * mention, and it is enough.
 *
 * The type is smaller than the 88px the old tagline carried, because this one
 * is half again as long and has to hold **one line**: `nowrap` is deliberately
 * *not* used here, since a horizontal overflow on the widest line in the deck
 * clips at the slide edge rather than showing up as the vertical crowding the
 * other pages fail with. The `maxWidth` leaves ~120 units of slack against the
 * ~1856 a slide has, so the line has somewhere to grow if the font ever
 * changes metrics.
 *
 * **The size is measured, not estimated.** At 72px this line renders 1598
 * units wide — one line, with ~130 units of slack against the `maxWidth` and
 * 145 units of clear space each side. It also holds one line at 76px (1687
 * units), which is what the tagline shipped at first; that runs the deck's
 * opening and closing headline nearly edge-to-edge, and 4px of type is the
 * cheaper thing to give up. Re-measure rather than re-estimate if the line
 * changes: load the page and read the rendered text width, because a character
 * count predicts this badly — the line that wrapped before was only ~8
 * characters longer than one that fits.
 */
const TitlePage = () => (
  <SlideBody centered>
    <Box margin="0 0 40px 0">
      <Wordmark width={860} />
    </Box>
    <Statement fontSize="72px" maxWidth="1730px">
      {TAGLINE}
    </Statement>
  </SlideBody>
);

export default function DemoDeck() {
  return (
    <Deck theme={deckTheme} template={template}>
      {/* 1 — Title */}
      <Slide>
        <TitlePage />
        {/* The tokenized-equity analogy is **spoken, never printed**. It is the
            sit-up moment — a VC who knows nothing about FX instantly gets that
            one asset class crossed onchain and the other didn't — and it plants
            page 10, which returns to tokenized equities with the number
            attached. On the slide it would be a second sentence competing with
            the tagline; said out loud over a bare wordmark, it lands. */}
        <Notes>
          Dropset is the liquidity layer for every national currency — all of
          them, for anyone, 24/7/365. Here’s what I mean. Anyone with a phone
          can buy tokenized Tesla stock today, from almost any country — and
          nobody can do that with a euro. That’s the hole we’re filling.
        </Notes>
      </Slide>

      {/* 2 — A huge market. The first half of the open: one number, and out. */}
      <Slide>
        <SlideBody>
          <Eyebrow>The market</Eyebrow>
          <Statement fontSize="72px">
            Foreign exchange is the biggest market on earth
          </Statement>
          {/* One figure, and deliberately nothing else. The v2 gap page carried
              this sentence plus six chevron-marked facts plus the meter plus a
              screenshot — accurate, and the densest thing in a deck that is
              otherwise concise. A pitch's first content page sets the pace for
              every page after it, so this one is a single number: huge market
              here, no penetration on page 3.

              Fragmentation and the 24/5 closing hours were two of those six
              facts and are now **spoken only**. Both are real and neither is
              the beat — the beat is the size of the prize — and as printed rows
              they had the audience reading a list while the presenter talked. */}
          <SlideFill>
            <HeroFigure figure="$9T+" caption="Traded every day" />
          </SlideFill>
        </SlideBody>
        <Notes>
          Foreign exchange is the biggest market on earth — over nine trillion
          dollars a day. It’s also structurally old: banks and over-the-counter
          desks fragment the liquidity, and it only trades 24/5 — it closes on
          Friday afternoon and it doesn’t open again until Sunday night.
        </Notes>
      </Slide>

      {/* 3 — No penetration. The second half of the open. */}
      <Slide>
        <SlideBody>
          <Eyebrow>The gap</Eyebrow>
          {/* Deliberately opens on "But", because this headline is the second
              half of page 2's. Read in sequence the two pages are one sentence
              — "Foreign exchange is the biggest market on earth" / "but it
              barely trades onchain" — which is what makes the split read as one
              open with momentum rather than as two market-size slides. It also
              beats the alternatives on precision: "blockchains have almost none
              of it" needs the audience to carry "it" across a slide boundary,
              and names the wrong subject (the page is about FX, not about
              blockchains). Keep the lowercase-after-"But" reading in mind if
              this is ever reworded — the pages only work as a pair. */}
          <Statement fontSize="72px">But it barely trades onchain</Statement>
          {/* The statement carries what used to be the fifth chevron fact, so
              this page needs no fact list at all: the meter shows the ratio,
              the screenshot cites it, and the sentence says what it means.
              That is what keeps this page as sparse as page 2 while still
              carrying the deck's most checkable number.

              Two claims that used to print here are now spoken. The 24/7/365
              goal was v2's one accent row, and the title page's spoken open
              still carries 24/7/365 — printing it again three pages later
              reads as the deck repeating itself rather than escalating. And
              the money-ness thesis ("public blockchains are the most money-like
              digital environment available today") was the abstract sentence on
              a page whose job is a concrete ratio; it is still the claim the
              deck answers to, and page 10 is its payoff. */}
          <SlideFill>
            <Box>
              <CurrencyMeter />
              <Screenshot
                src="/screens/currencies-listed.png"
                width={METER_WIDTH}
                alt="14 of 162 currencies represented on Solana; 148 not yet listed"
                source="dropset.io/currencies"
                margin="30px 0 0 0"
              />
            </Box>
          </SlideFill>
        </SlideBody>
        {/* `LISTED_CURRENCIES` / `TOTAL_CURRENCIES` drive both the meter and the
            spoken figure below, and they are live numbers from our own site —
            check them before presenting. A stale slide beside a corrected
            number read aloud is worse than either alone. */}
        <Notes>
          And blockchains have almost none of it. Less than ten percent of the
          world’s currencies are on Solana — fourteen out of a hundred and
          sixty-two — and that count is live on our own site, which is where
          this is from. Public blockchains are the most money-like digital
          environment we have, and Solana most of all: it’s the fastest and the
          cheapest. So the gap is the whole opportunity. Our goal is 24/7/365
          coverage of every FX spot pair — every currency connectable to every
          other one. To be precise: we don’t issue currencies — issuers create
          them, and Dropset is where they trade.
        </Notes>
      </Slide>

      {/* 4 — Live today */}
      <Slide>
        <SlideBody>
          <Eyebrow>Live today</Eyebrow>
          {/* One line, enforced rather than estimated. It is the cheapest 72
              units on the page and the captures need every one — see
              `STEP_WIDTH`. */}
          <Statement fontSize="60px" nowrap>
            Dropset already processes Solana mainnet FX trades
          </Statement>
          {/* The swap flow left to right, one step per column — the beat that
              used to be the mainnet recording. The three captures are very
              different heights, and centring them vertically is what makes that
              read: each caption sits directly above its own capture, so the
              labels climb like steps as the captures grow taller. The chevrons
              inherit the same centring, landing on the row's axis rather than
              pinned to the top of the tallest column.

              The margin above the row is 16, down from this page's own 36 and
              the tightest such gap in the deck: this is the densest page, so
              the space between the sentence and the captures is the cheapest
              thing on it to give up, and buying height here is what keeps the
              eyebrow off the top edge. (These gaps are per-page — they run 16
              to 46 across the deck — so there is no single value to be
              consistent with.)

              Deliberately *not* symmetric with the 10 above the sentence, even
              though these are the two gaps that bracket it. A kicker belongs to
              the sentence under it, so it should sit closer to that sentence
              than the sentence sits to the page's content — matching the two
              would read as three unrelated bands. Make them equal here if that
              ever looks wrong; the totals are what the budget cares about. */}
          <FlexBox margin="16px 0 0 0" justifyContent="center" alignItems="center">
            <Step
              step={1}
              label="Open the picker"
              src="/screens/swap-picker.png"
              width={STEP_WIDTH}
              alt="The swap panel's To field, choosing the currency to receive"
            />
            <SequenceArrow />
            {/* The URL rides under the middle step rather than under the whole
                row. The third capture is by far the tallest, so a link below the
                row sat almost on the footer — and it cost the page 64 units of
                height it did not have. In the space the shorter middle column
                leaves, it costs nothing and lands under the flow's centre. */}
            <Box>
              <Step
                step={2}
                label="Select your currency"
                src="/screens/swap-search.png"
                width={STEP_WIDTH}
                alt="Searching for a currency and its available stablecoins"
              />
              <FlexBox margin="30px 0 0 0">
                <Link href="https://dropset.io/swap" fontSize="28px">
                  dropset.io/swap
                </Link>
              </FlexBox>
            </Box>
            <SequenceArrow />
            <Step
              step={3}
              label="Swap atomically"
              src="/screens/swap-settled.png"
              width={STEP_WIDTH}
              alt="A priced USDC to EURC swap, with the route drawn on the globe"
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          This already works. Dropset already processes Solana mainnet FX
          trades: you open the picker, select your currency, and the swap settles
          atomically. The ramps are near instant and the venue never
          closes. Solana is the start, not the end — it’s the most
          moneyness-conducive environment onchain. And it’s on dropset.io/swap
          right now, so you can go and do this yourself. [Today we clear by
          routing through aggregators and sourcing existing liquidity; don’t
          claim “most liquid”.]
        </Notes>
      </Slide>

      {/* 5 — Currency curation. A continuation of "live today". */}
      <Slide>
        <SlideBody>
          <Eyebrow>Currency curation</Eyebrow>
          <Statement fontSize="56px" nowrap>
            Dropset curates market data for all Solana-based currencies
          </Statement>
          {/* One capture, as large as the page allows. An earlier version put
              three tables on this page and none of them could be read. 1100 is
              the cap, not a preference: at 1150 this page sat ~36 units off
              overflowing, which is inside the error bar on text metrics. */}
          <Screenshot
            src="/screens/currencies-by-liquidity.png"
            width={1100}
            alt="Every currency on Solana sorted by on-chain liquidity, deepest first, with price, 24h volume, market cap and holders"
            margin="30px 0 0 0"
          />
        </SlideBody>
        <Notes>
          And alongside the swap itself, Dropset curates the market data for
          every Solana-based currency: price, twenty-four-hour change and volume,
          market
          cap, liquidity, holders — grouped by country, or sorted however you
          want. This is sorted by liquidity, deepest first.
        </Notes>
      </Slide>

      {/* 6 — The illiquid tail. The problem the eCLOB answers. */}
      <Slide>
        <SlideBody>
          <Eyebrow>The long tail</Eyebrow>
          <Statement fontSize="64px" nowrap>
            Many currencies have no liquidity whatsoever
          </Statement>
          <Screenshot
            src="/screens/currencies-illiquid.png"
            width={1400}
            alt="The tail of the same table: the Australian and Canadian dollars, the yen, the naira, the lira and more, all with no price, volume or liquidity at all"
            margin="34px 0 0 0"
          />
        </SlideBody>
        <Notes>
          Scroll to the bottom of that same list and the story tells itself. The
          Australian dollar, the Canadian dollar, the yen, the naira, the lira —
          all sitting there with no price, no volume, and no liquidity at all.
          These are real currencies with real economies behind them, and onchain
          they have no market whatsoever.
        </Notes>
      </Slide>

      {/* 7 — The eCLOB */}
      <Slide>
        <SlideBody>
          <Eyebrow>The eCLOB</Eyebrow>
          {/* One sentence, not a statement plus a supporting line: the eyebrow
              above already names the eCLOB, so the sentence goes straight to
              what it ships. "CLOB", not "order-book" — the acronym, since the
              page has already named the thing. The compute-unit numbers live on
              the capture that shows them rather than being restated here. */}
          {/* Pinned to one line by `nowrap`, not by a width estimate. Every
              earlier attempt at this heading wrapped further than intended, and
              the page's own overflow then clipped this slide's eyebrow off the
              top — three review rounds running. Short copy plus `nowrap` ends
              that: the browser now enforces what the budget assumes. */}
          <Statement fontSize="56px" nowrap>
            Dropset ships propAMM efficiency and CLOB transparency
          </Statement>
          {/* Three captures side by side rather than two stacked in a column:
              that is what gives this page its headroom, and it still reads
              left to right as cost → maker → product without needing chevrons
              to say so. Vertically centred, so the captures share a midline
              despite being very different heights. */}
          <FlexBox margin="36px 0 0 0" justifyContent="center" alignItems="center">
            <Box margin="0 20px">
              <Screenshot
                src="/screens/compute-units.png"
                width={500}
                alt="Compute units per instruction: a reprice costs 47, a reshape 59"
                caption="Reprice: 47 CU · reshape: 59 CU"
                margin="0px"
              />
            </Box>
            <Box margin="0 20px">
              <Screenshot
                src="/screens/maker-tui.png"
                width={500}
                alt="The maker control panel: seven FX markets and a live book"
                caption="Demo maker quoting locally"
                margin="0px"
              />
            </Box>
            <Box margin="0 20px">
              <Screenshot
                src="/screens/eclob-frontend.png"
                width={500}
                alt="The eCLOB on the frontend: a EURC/USDC order book, a live trades tape, and a priced swap"
                caption="Liquidity routes to the frontend"
                margin="0px"
              />
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          So we’re building the exchange those markets need. Making a market
          onchain used to be prohibitively expensive — gas made continuous
          quoting impossible, so everything before this was a band-aid. We’ve
          built order books before, so we built one that fits: the eCLOB gives
          you the transparency of a central limit order book with quote updates
          as cheap as a propAMM. Repricing the whole book costs forty-seven
          compute units and reshaping the ladder fifty-nine, on a chain that
          gives you two hundred thousand per instruction. Left to right: that’s
          what a quote costs, that’s our own maker paying it to quote a live
          market, and that’s the same liquidity arriving on the frontend with the
          book, the trades tape and a priced swap. We’re building this out so
          anyone can quote onchain with a vault-style approach.
        </Notes>
      </Slide>

      {/* 8 — How we grow */}
      <Slide>
        <SlideBody>
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="68px">
            FX vaults bootstrap a public liquidity flywheel
          </Statement>
          <SlideFill>
            <Flywheel />
          </SlideFill>
        </SlideBody>
        <Notes>
          We seed the markets ourselves, the way every venue that ever
          bootstrapped its own liquidity did — our vaults bootstrap each book,
          and anyone can top them off, so the flywheel is
          public rather than ours alone. The wedge is that long tail of
          currencies: spreads are wide there, and an issuer arriving with no
          depth of their own needs a day-one liquidity partner. And it’s a
          two-sided market we’re already doing the customer development on.
          Upstream are the stablecoin issuers — AUDD Digital, and Loon, who
          issues CADC — who mint a currency and need it to trade. Downstream is
          the demand: Altitude in banking, CargoBill in supply chain, who need to
          buy FX to settle. Connect the two ends and the depth compounds.
        </Notes>
      </Slide>

      {/* 9 — Growth roadmap */}
      <Slide>
        <SlideBody>
          <Eyebrow>Growth roadmap</Eyebrow>
          {/* The statement names the path's **destination**, which is what makes
              this the deck's opportunity page: the beats below start at seeding
              a handful of pairs, and without the endpoint stated an audience has
              no reason to think that adds up to anything. It replaced "our path
              to expansion is deliberate and methodical", which described the
              manner of the plan and not where it goes. "And beyond" is carrying
              the Later beat — the destination is every currency trading around
              the clock, and the product doesn't stop once it gets there.
              Naming the near end too ("from bootstrapping liquidity to…") was
              tried and cost a second line to restate what the first beat says
              two inches below.

              This is the deck's **one fragment headline**, and it is deliberate.
              A path is a noun, the page is a timeline, and the sentence forms
              ("our path runs to…") all read as a hedge about the destination
              rather than as the destination. */}
          <Statement fontSize="68px">The path to 24/7/365 FX and beyond</Statement>
          <SlideFill>
            <Roadmap />
          </SlideFill>
        </SlideBody>
        <Notes>
          This is the path to 24/7/365 FX, and beyond it. Now, we onboard
          emerging stablecoin issuers: we lead the vaults that give them day-one
          liquidity, and at the same time we develop the downstream pipeline of
          the companies and users who need liquid currency swaps — those are the
          two sides of the market. Next, protocol fees accrue value: this isn’t a
          prop AMM, so as additional market makers come in, quotes get tighter,
          volume follows the tighter spreads, liquidity compounds, and protocol
          fees accrue value off it. Later, the product expands
          beyond spot — derivatives make the markets themselves more efficient,
          and they open up business use cases too: treasury management, hedging
          B2B payment flows. Hedging isn’t just for market makers.
        </Notes>
      </Slide>

      {/* 10 — Why FX is next */}
      <Slide>
        <SlideBody>
          <Eyebrow>Why FX is next</Eyebrow>
          {/* The deck's thesis line, kept from v2 — it is the sentence the
              whole argument has been walking toward, and the continuous read
              closes on it. An intermediate v3 draft replaced it with "New asset
              classes consolidate where liquidity is public", which states the
              *pattern* instead; that claim is real but it belongs under the
              row, as the reading of the evidence. The headline should be the
              conviction, not the observation.

              Pinned to one line at 60, exactly as v2 had it: it fits the
              measure, and the row below leaves nothing to spend on a second. */}
          <Statement fontSize="60px" nowrap>
            Public liquidity is what blockchains were built for
          </Statement>
          <SlideFill>
            <Box>
              <Flip />
              {/* The page's whole spend on the argument, and it has to stay one
                  line. It carries the claim the headline gave up when that went
                  back to the thesis line: the tiles are three facts, and this is
                  what reading them together means.

                  **"Bootstrap", not "consolidate".** An earlier draft read
                  "Each new asset class consolidates where anyone can quote",
                  which describes where the asset classes ended up — a
                  restatement of the row directly above it. This says what the
                  environment *did*: an open venue is what lets a new asset
                  class get started at all, which is the training-wheels
                  argument the first tile makes and the reason the third one
                  follows. It also puts the environment in the subject
                  position, so the sentence is about the property rather than
                  about the assets.

                  A second line turns the flip back into the wind-up it
                  replaced. */}
              <div
                style={{
                  color: colors.accent,
                  fontSize: "34px",
                  lineHeight: 1.3,
                  marginTop: "44px",
                  textAlign: "center",
                }}
              >
                Open environments bootstrap new asset classes
              </div>
            </Box>
          </SlideFill>
        </SlideBody>
        {/* **No competitor is named here, and none may be added.** The spoken
            track says "permissioned rails" and nothing more specific. The
            argument is unchanged from v2 but is made as a *property* claim —
            gated access can't compound liquidity — which is stronger than an
            attack because it is about a mechanism rather than about a company.

            Two things survive here from the page this replaced. The **"today"**
            qualifier on Solana is now spoken only, and it still has to be said:
            it is the answer to what happens if a better settlement layer
            arrives. And the **absorption** claim — a maker with gated access
            can quote here and hedge there, and nothing carries depth back —
            repairs a soft spot on page 8, which otherwise leaves our own vaults
            as the only answer to where depth for a thin pair comes from. Say it
            as a capability, not as something running today. The old "vampire
            attack" phrasing is retired: with no competitor named it has no
            referent, and the term properly describes poaching an incumbent's
            liquidity providers with incentives, which is a different mechanic.

            The RWA figures are spoken because their honest form is about
            *growth*, not size — Solana is third by total RWA value behind
            Ethereum and BNB, so any "Solana leads RWAs" claim hands a listening
            investor a free correction. */}
        <Notes>
          So why does FX land here, and why now? Because every new asset class
          has already consolidated on Solana, in order. First memecoins — over
          twelve million tokens launched. Call that the training wheels: it
          looks unserious, and it was the proving ground. It established that
          anyone could launch a market on this chain and anyone could trade
          against it, at a scale nothing else came close to. And it is a real
          business — one launchpad alone became the number-one
          DEX by daily volume across every chain, on a billion dollars of
          cumulative revenue. Then tokenized equities, and that one is a big
          deal: ninety-six percent of onchain tokenized-equity
          volume is on Solana, and effectively nowhere else. Real-world assets
          are going the same way — Solana added more of them in the last six
          months than in its whole history before that, and it has more holders
          of them than any other chain. So the training wheels came off. FX is
          next, and it’s the biggest of
          them. It lands here for the same reason the others did: public
          liquidity. Anyone can quote, anyone can trade against it, and it
          composes with everything else onchain — so depth compounds instead of
          sitting still. Solana is the most money-like onchain environment
          today, and I say today deliberately: it’s where this belongs right now
          because it’s the fastest and the cheapest, not a commitment we’re
          locked into if something better shows up. What we’re building is
          public liquidity, and that’s portable. That’s the part the
          permissioned rails structurally can’t have: gate who gets to make a
          market and liquidity never compounds, it just sits where you put it.
          And it runs one way — a maker who already has access to one of those
          venues can quote here and hedge there, so their depth reaches a public
          book, and nothing carries it back. Public liquidity is what
          blockchains were built for — moving money is the problem they were
          supposed to solve, and this is that.
        </Notes>
      </Slide>

      {/* 11 — Team. No longer the last page: the close (12) replays the title
          and is what stays up, so the mirror-the-title line and the
          "leave this page up" direction both moved there. */}
      <Slide>
        <SlideBody>
          <Eyebrow>The team</Eyebrow>
          <Statement fontSize="64px">
            Dropset is built by people who have built exchanges
          </Statement>
          {/* 32, down from this page's own 46, for the same reason page 4 came
              down to 16: with two five-line bios under two headshots, this is
              the second densest page, and the gap above the portraits is what
              was pushing the eyebrow into the top edge. */}
          <FlexBox
            margin="32px 0 0 0"
            justifyContent="center"
            alignItems="flex-start"
          >
            {/* Roles carry **no company**. "Founder, DASMAC" named the DevCo a
                third time on a page that is about people, and the deck now names
                it once, in the footer. The bio leads with the full-stack claim —
                the whole stack, on more than one chain — because that is the
                thing the two named credentials are evidence *of*, and stating it
                first stops them reading as two unrelated projects. */}
            <Portrait
              src="/remote/team-alex.png"
              name="Alex Kahn"
              role="Founder"
              prior="prev. Cofounder, Econia Labs"
              bio="Authored exchange technology across the entire stack on multiple blockchains, including the Econia order book on Aptos ($500M lifetime volume) and the Solana Opcode Guide, a key resource for optimizing Solana program efficiency."
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy Sosa"
              role="Operations"
              prior="prev. EA, Dragonfly Capital"
              bio="Owns the whole operational stack, working with banks, stablecoin providers, onramps and service providers. Extensive background in logistical coordination and partner relationship management."
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          Dropset is built by people who have built exchanges. I’ve authored
          exchange technology across the entire stack, on more than one
          blockchain — the Econia order book on Aptos, five hundred million
          dollars of lifetime volume, and the Solana Opcode Guide, a key resource
          for optimizing Solana program efficiency, which is what makes
          quoting on the eCLOB cost double-digit compute units. Judy owns the
          whole operational stack, and works directly with banks, stablecoin
          providers, onramps and service providers, on an extensive background in
          logistical coordination and partner relationship management.
        </Notes>
      </Slide>

      {/* 12 — Close. A replay of page 1, and the page that stays up. */}
      <Slide>
        <TitlePage />
        {/* This page exists because the sign-off review was left **wondering
            why we care about this**, and observed that people invest in a
            founder at least as much as in a problem. v2 ended on the team page,
            which states credentials — it answers "can they build it", not "why
            are they the ones who will". Putting the why-me over a replay of the
            title lands it on the deck's own thesis rather than over two
            headshots, and it gives the talk a bookend: the tagline is the first
            thing said and the last.

            The personal why below is the founder's own argument, written down
            as a **rehearsal draft** rather than as copy to be read out — it is
            the one block in this deck meant to be iterated on out loud, and
            the wording that lands will be whatever it becomes on the third
            pass. Keep it to two or three sentences. The failure mode is a
            biography; the target is the single sentence that makes a listener
            believe this person would still be working on this in five years. */}
        <Notes>
          Dropset — the liquidity layer for every national currency. This
          industry is seventeen years old now, and we can send money around
          the world just fine — what we still can’t do is get in and out of
          currencies. There’s no shortage of ways to speculate onchain and no
          shortage of alternative stores of value, but the currencies people
          actually earn and spend in don’t flow on a decentralized ledger the
          way it promised they would. A large part of why is that there’s no
          FX liquidity onchain — and Solana is the most money-like environment
          we’ve ever had, which is why this is where it gets fixed. [Leave
          this page up.]
        </Notes>
      </Slide>
    </Deck>
  );
}
