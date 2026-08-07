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
 *
 * Ten pages, and pages 3–6 are one argument in sequence: the swap flow
 * works today, we curate the data for every currency, most of them have no
 * liquidity at all, and the eCLOB is what we're building to fix that. Then how
 * we grow, the roadmap, why an open venue wins, and the team — which is last
 * and stays up after the talk.
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
        the mark by taking the label *out of flow* works horizontally but loses
        this row's vertical centring, at which point the theme-scale margin trap
        `CreditLabel` describes drops the label below the mark's baseline.
        Everything stays in flow here. */}
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
 * How much vertical room the footer is kept clear of, in slide units.
 *
 * The footer is absolutely positioned over the bottom of the slide, so the body
 * has to reserve space for it or the lowest element on a busy page crowds the
 * DASMAC credit. What matters is that the reserve is **split across both ends**
 * rather than all taken off the bottom, which is how it started and is subtly
 * wrong: content is centred inside whatever is left, so a bottom-only reserve
 * put the box's centre ~53 units above the slide's own centre and every page
 * rode high. It showed worst on the two densest pages — 3 and 10 — where there
 * is almost no slack left to centre, so the eyebrow crowded the top edge while
 * a wide empty band sat under the content.
 *
 * Splitting it costs no page a single unit, which is the reason to prefer it
 * over trimming margins page by page: the available height is still
 * `1016 - FOOTER_RESERVE` either way, so every height budget below still holds
 * and no page can newly overflow — the box simply sits where the slide's centre
 * is. Half the reserve still clears the footer comfortably; the footer is ~42
 * units tall (its 210-wide wordmark sets that), and on the tightest page content
 * now ends ~55 units above it.
 */
const FOOTER_RESERVE = 106;

/**
 * Every slide's content column.
 *
 * Between this and Spectacle's own 32px slide padding, a page has ~910 of the
 * 1080 slide units to work in. Worth doing the arithmetic when adding to a
 * page: content that overflows is merely scaled down on screen but **silently
 * clipped in print**, which is the path to the meta-deck.
 */
const SlideBody = ({ children }: { children: React.ReactNode }) => (
  <FlexBox
    height="100%"
    flexDirection="column"
    justifyContent="center"
    padding={`${FOOTER_RESERVE / 2}px 0`}
  >
    {children}
  </FlexBox>
);

/**
 * Small monospace kicker that labels each content slide. Uppercase, letterspaced
 * Space Mono — the exact treatment the company's own tag carries ("DISTRIBUTED
 * ATOMIC STATE MACHINE ALGORITHMS CORPORATION"), so the deck's kickers and the
 * brand's typography are visibly the same system even now that the company
 * banner itself is off the deck. Sentence case stays with Inter, where it
 * belongs.
 *
 * It sits **close** to the sentence it labels — tight bottom margin, and an
 * explicit `lineHeight`, because `Text` sets none and would otherwise inherit
 * the browser's `normal` (~1.21) and hang leading under a single line of 26px
 * type, widening a gap the margin only appears to control. A kicker should read
 * as a label on the sentence rather than as a line of its own, and on the
 * densest pages every unit between the two is height the captures want.
 */
const Eyebrow = ({ children }: { children: React.ReactNode }) => (
  <Text
    color="secondary"
    fontFamily="monospace"
    fontSize="26px"
    margin="0 0 10px 0"
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
 * The six supporting facts on the gap page.
 *
 * `justifyContent` is the load-bearing prop here, and it must stay explicit:
 * Spectacle's `FlexBox` **defaults to `center`**, which centred each row
 * independently — so the short one-line fact sat visibly inset from the wrapped
 * two-line ones, reading as a stray indent rather than a list.
 *
 * The marker is a chevron rather than a disc: a row of discs is what makes a
 * slide read as a corporate template. It's the angular "greater-than" shape the
 * review asked for, but deliberately *not* a literal `≥` — that glyph makes a
 * numeric claim, and it would flatly contradict the last fact, which is a
 * *less-than*.
 *
 * The row spacing is the page's give. This started as three facts at 26 units
 * apart; at six, that spacing made the list overrun the meter column beside it
 * for no gain in legibility, so it came down. If the page ever overflows again,
 * this number is the first place to take it from.
 *
 * `accent` is for the **last** fact, the ambition. It is the one row that isn't
 * a fact about the world as it is, and in one flat color it read as a trailing
 * qualifier on the statistic above it rather than as the thing the deck is
 * actually going after. The accent lifts it out with no size change and no
 * layout of its own, and it's the color the meter's fill and every chevron on
 * the page already carry — so it reads as emphasis, not as a new kind of thing.
 */
const Fact = ({
  accent = false,
  children,
}: {
  accent?: boolean;
  children: React.ReactNode;
}) => (
  <FlexBox
    alignItems="flex-start"
    justifyContent="flex-start"
    margin="0 0 20px 0"
  >
    <div
      style={{
        color: colors.accent,
        flex: "0 0 auto",
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "34px",
        lineHeight: 1.25,
        marginRight: "20px",
      }}
    >
      ›
    </div>
    <div
      style={{
        color: accent ? colors.accent : colors.foreground,
        fontSize: "34px",
        lineHeight: 1.25,
      }}
    >
      {children}
    </div>
  </FlexBox>
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
const METER_WIDTH = 760;
const LISTED_SHARE = (LISTED_CURRENCIES / TOTAL_CURRENCIES) * 100;

const CurrencyMeter = () => (
  // Inset to match the screenshot mounted under it: that capture sits inside a
  // bordered, padded frame, so its image starts `SCREENSHOT_INSET` in from the
  // frame's outer edge. Without the same inset the bar and the figure it cites
  // are a padding-width out of true with each other.
  <Box width={`${METER_WIDTH}px`} margin={`0 ${SCREENSHOT_INSET}px`}>
    <FlexBox justifyContent="space-between" alignItems="flex-end">
      <div style={{ color: colors.mutedFg, fontSize: "26px" }}>
        Currencies available on Solana
      </div>
      <div
        style={{
          color: colors.accent,
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "30px",
        }}
      >
        {LISTED_SHARE.toFixed(1)}%
      </div>
    </FlexBox>
    <div
      style={{
        backgroundColor: colors.meterTrack,
        borderRadius: "16px",
        height: "32px",
        marginTop: "14px",
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
 * A screenshot frame's chrome, named because anything stacked above or below a
 * capture has to inset by the same amount to line up with the image rather than
 * with the frame's outer edge — see `SCREENSHOT_INSET` and the gap page's meter.
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

// The chevron between steps of the swap flow. Space Mono, matching the same
// glyph used as the `Fact` marker — one mark, one face.
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
 * This is the number that keeps page 3 inside its slide, so it is a named
 * constant rather than repeated at three call sites. The third capture is very
 * tall (820×1371), so it sets the row's height, and the row's height is most of
 * the page: at 410 — with a two-line statement and the URL below the row — the
 * page stacked to ~1008 units against the ~910 a slide has, and flex centring
 * pushed the eyebrow off the top edge (cropped on screen, silently clipped in
 * print).
 *
 * Widening it back to 420 is only safe because two other things now hold: the
 * statement is pinned to one line by `nowrap` (the guarantee — `maxWidth` is
 * only an estimate), and the URL moved into the middle column. Those are
 * load-bearing — undo either and this has to come back down.
 */
const STEP_WIDTH = 420;

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
  showNames = true,
}: {
  logos: Logo[];
  width?: number;
  height?: number;
  tint?: string;
  margin?: string;
  showNames?: boolean;
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

            `showNames` is off only where **every mark in the row is its own
            wordmark**, as on the open-venue page — a name under a logotype that
            already says the name is just the word twice. Keep it on wherever a
            mark is an icon rather than a wordmark (the flywheel's are), since
            those say nothing on their own. The `name` still does real work when
            hidden: it is the `alt` text and the row's key.

            The one thing this loses: `PERMISSIONED`'s first mark is **Circle's**,
            because Arc is Circle's chain, so that chip now reads as Circle while
            the spoken track says Arc. Accepted deliberately — Circle is the
            entity an audience recognizes, and the page's argument is about
            private ledgers generally rather than about Arc in particular. */}
        {showNames ? (
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
        ) : null}
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

/**
 * The permissioned rails, on the open-venue page. Alphabetical, so the row
 * implies no ranking among them, and tinted with the sell color — these are
 * the unfavorable case the page argues against.
 *
 * These render **uncaptioned** (`showNames={false}`), because each mark is a
 * wordmark that already says the company. The `name` here is still the `alt`
 * text and the key, and it is the name the *presenter* says — which isn't always
 * what the mark says, since Arc is Circle's chain and Circle's logo is the one
 * an audience recognizes. So this row reads Circle / Canton / Tempo on-slide
 * against "Arc and Tempo" spoken, which is fine: the argument is about private
 * ledgers as a class, not about Arc specifically. Restore the captions if the
 * page ever needs to name one in particular.
 */
const PERMISSIONED: Logo[] = [
  { name: "Arc", src: "/remote/logo-circle.svg" },
  { name: "Canton", src: "/remote/logo-canton.svg" },
  { name: "Tempo", src: "/remote/logo-tempo.svg" },
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
type Beat = { when: string; headline: string; body: string };

const BEATS: Beat[] = [
  // Plain-spoken customer development, and it **names no company**: an earlier
  // draft read "DASMAC bootstraps liquidity", which put the DevCo back on a
  // slide it has receded from and made the first beat about us rather than
  // about the issuers. Both sides of the two-sided market belong here — the
  // issuers upstream and the pipeline of companies that consume the liquidity
  // downstream (the page-7 names) — because seeding one side is a liquidity
  // operation, and seeding both is a market.
  {
    when: "Now",
    headline: "We onboard emerging stablecoin issuers",
    body: "We lead the vaults that give an emerging issuer day-one liquidity, and we develop the downstream pipeline of companies and users who need liquid currency swaps.",
  },
  // Other market makers entering is the load-bearing half, and it is what says
  // this **isn't a prop AMM**: anyone can quote here, so quotes compete tighter
  // rather than being set by whoever owns the venue. That competition is the
  // mechanism the compounding runs on — spreads tighten, volume follows,
  // liquidity deepens — and only then do fees mean anything. The fee clause
  // stays in abstracted language; naming the switch turns a growth beat into a
  // token-design question.
  {
    when: "Next",
    headline: "Protocol fees accrue value",
    body: "As additional market makers enter, quotes get tighter, and protocol fees accrue value as volume and liquidity compound.",
  },
  // The headline stays broad on purpose — "beyond spot" lets a reader fill in
  // their own derivatives thesis, where naming one hands them ours to argue
  // with. Hedging is the through-line, and deliberately not only a market
  // maker's tool: the two named business cases are what says so, and they are
  // examples rather than a list — the beat is that derivatives buy both a
  // better market structure and downstream uses of it.
  {
    when: "Later",
    headline: "Product expansion beyond spot",
    body: "Derivatives enable more efficient markets and business use cases, including treasury management and hedging B2B payment flows.",
  },
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
      {BEATS.map(({ when, headline, body }) => (
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
          <div
            style={{
              color: colors.foreground,
              fontSize: "34px",
              lineHeight: 1.25,
              marginTop: "14px",
            }}
          >
            {headline}
          </div>
          <div
            style={{
              color: colors.mutedFg,
              fontSize: "24px",
              lineHeight: 1.4,
              marginTop: "14px",
            }}
          >
            {body}
          </div>
        </Box>
      ))}
    </FlexBox>
  </Box>
);

/**
 * One claim under an open-venue panel.
 *
 * The panel captions are **bullets, not prose blocks** — the page's second
 * sanctioned break from "no bullet lists", after the gap page's facts. Two
 * paragraphs of small text under two badges read as fine print nobody finishes;
 * two short columns of peer claims read as an argument against an argument,
 * which is what this page is.
 *
 * The chevron takes its **panel's own tint** rather than the deck accent, so a
 * claim belongs visibly to its side of the comparison — the two columns are read
 * across as often as down, and the marker is what keeps a red claim from being
 * skimmed as one of ours.
 */
const VenueBullet = ({
  tint,
  children,
}: {
  tint: string;
  children: React.ReactNode;
}) => (
  <FlexBox
    alignItems="flex-start"
    justifyContent="flex-start"
    margin="0 0 14px 0"
  >
    <div
      style={{
        color: tint,
        flex: "0 0 auto",
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "27px",
        lineHeight: 1.35,
        marginRight: "14px",
      }}
    >
      ›
    </div>
    <div style={{ color: colors.foreground, fontSize: "27px", lineHeight: 1.35 }}>
      {children}
    </div>
  </FlexBox>
);

/**
 * One side of the open-venue comparison: a bordered panel with its claims under
 * it. The border color is the whole argument — the permissioned side is tinted
 * with the sell red, the Dropset side with the buy green — so the page reads
 * before any of its copy does.
 *
 * Claims come in as **strings, not markup**, so the panel applies its own tint
 * to every one of its bullets. Passing rendered bullets instead would mean
 * repeating the tint at each call site, where one stale value would put a green
 * chevron in the red column and quietly reverse which side the reader thinks a
 * claim belongs to.
 *
 * Four fits, but only just, and only since the footer reserve was split and the
 * headline's phantom margin removed — together those gave this page back ~85
 * units, where a fourth two-line bullet costs ~85. At four green and three red
 * the page lands near 735 of its ~910. A fifth needs height found elsewhere
 * first, and the columns are already visibly uneven, which is the real limit:
 * the panels read as a comparison only while both sides look like sides.
 */
const VenuePanel = ({
  tint,
  claims,
  children,
}: {
  tint: string;
  claims: string[];
  children: React.ReactNode;
}) => (
  <Box width="700px" margin="0 26px">
    <FlexBox
      alignItems="center"
      justifyContent="center"
      border={`2px solid ${tint}`}
      borderRadius="16px"
      height="210px"
    >
      {children}
    </FlexBox>
    <Box margin="20px 0 0 0">
      {claims.map((claim) => (
        <VenueBullet key={claim} tint={tint}>
          {claim}
        </VenueBullet>
      ))}
    </Box>
  </Box>
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

export default function DemoDeck() {
  return (
    <Deck theme={deckTheme} template={template}>
      {/* 1 — Title */}
      <Slide>
        <SlideBody>
          <Box margin="0 0 40px 0">
            <Wordmark width={860} />
          </Box>
          {/* The wordmark and one sentence, and nothing else. This page used to
              carry a "Built by DASMAC" line and the wide company banner beneath
              it; both are gone. DASMAC is the boring DevCo in the background —
              someone finds it when they sign a document — so a title slide that
              argues for it argues for the wrong thing. The footer credit is the
              deck's one mention, and it is enough. */}
          <Statement fontSize="88px">Where currency trades onchain</Statement>
        </SlideBody>
        <Notes>Dropset is where currency trades onchain.</Notes>
      </Slide>

      {/* 2 — The gap */}
      <Slide>
        <SlideBody>
          <Eyebrow>The gap</Eyebrow>
          <Statement fontSize="72px">
            Foreign exchange is the biggest market on earth
          </Statement>
          <FlexBox
            margin="44px 0 0 0"
            alignItems="flex-start"
            justifyContent="flex-start"
          >
            {/* Six facts, and the order is the page's argument: a huge market,
                two structural problems with it, the environment that fixes
                exactly those, the gap that's still left, and what we're going
                after. Fact 3 says the closing hours plainly and does not absorb
                the OTC desks — fragmentation is fact 2's job, and folding them
                together loses one of the two problems. Fact 4 is the thesis the
                rest of the deck answers to, and it names public blockchains
                rather than Solana, because the claim is about the class of
                environment; "especially Solana" is a spoken line.

                The gap and the goal are **two rows, not one**. Joined by a dash
                they read as one sentence whose second half qualifies the first,
                which is exactly backwards: the goal is the bigger claim, and it
                has to be able to be read on its own. */}
            {/* 880, up from 800. The row is centred as a block and the meter
                column beside it is ~798 wide, so at a 60-unit gutter this still
                totals ~1738 of the ~1856 a slide has — the widening is free, and
                a wider measure is what keeps six facts from each taking a line
                more than they need. */}
            <Box width="880px" margin="0 60px 0 0">
              <Fact>Daily volumes exceed $9 trillion</Fact>
              <Fact>Banks and OTC desks fragment liquidity</Fact>
              <Fact>FX markets only trade 24/5</Fact>
              <Fact>
                Public blockchains are the most money-like digital environment
                available today
              </Fact>
              {/* "are on", not "are available on": this row has to land in two
                  lines. It is the longest fact on the page now that it names
                  what Solana is best at, and at three lines it reads as a
                  paragraph among statements. Trim this clause first if it ever
                  grows again — the Solana qualifier is the part doing new work. */}
              <Fact>
                Less than 10% of the world’s currencies are on Solana, the
                fastest and cheapest chain
              </Fact>
              <Fact accent>Our goal is 24/7/365 coverage of every FX spot pair</Fact>
            </Box>
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
          </FlexBox>
        </SlideBody>
        <Notes>
          Foreign exchange is the biggest market on earth — over nine trillion
          dollars a day. But banks and over-the-counter desks fragment its
          liquidity, and it only trades 24/5 — it closes on Friday. Public
          blockchains are the most money-like digital environment we have, and
          Solana most of all — it is the fastest and the cheapest. And yet less
          than ten percent of the world’s currencies are available there: fourteen
          out of a hundred and sixty-two,
          and that count is live on our own site, which is where this is from. Our
          goal is 24/7/365 coverage of every FX spot pair — every currency
          connectable to every other one, and that’s what we’re building. To be
          precise: we don’t issue currencies — issuers create them, and Dropset is
          where they trade.
        </Notes>
      </Slide>

      {/* 3 — Live today */}
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

              The margin above the row is tighter than the deck's usual 36: this
              is the densest page, so the space between the sentence and the
              captures is the cheapest thing on it to give up, and buying height
              here is what keeps the eyebrow off the top edge.

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

      {/* 4 — Currency curation. A continuation of "live today". */}
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

      {/* 5 — The illiquid tail. The problem the eCLOB answers. */}
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

      {/* 6 — The eCLOB */}
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

      {/* 7 — How we grow */}
      <Slide>
        <SlideBody>
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="68px">
            FX vaults bootstrap a public liquidity flywheel
          </Statement>
          <Flywheel />
        </SlideBody>
        <Notes>
          We seed the markets ourselves the way Hyperliquid did — our vaults
          bootstrap each book, and anyone can top them off, so the flywheel is
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

      {/* 8 — Growth roadmap */}
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
          <Roadmap />
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

      {/* 9 — Why the open venue wins */}
      <Slide>
        <SlideBody>
          <Eyebrow>Why the open venue wins</Eyebrow>
          {/* The page is **the long-term question** — public or private onchain
              liquidity — and this states our side of it rather than the other
              side's problem. It replaced "permissioned onchain liquidity has an
              adoption ceiling", which made the page about them; the rails are
              context for the question now, not targets. Pinned to one line:
              at 60 it fits the measure, and the panels below leave nothing to
              spend on a second. */}
          <Statement fontSize="60px" nowrap>
            Public liquidity is what blockchains were built for
          </Statement>
          {/* `alignItems` must stay explicit. Spectacle's `FlexBox` defaults to
              `center`, which vertically centred each panel *including its
              caption* — so the panel with the longer caption was the taller
              column, and its badge rode up relative to the other one. Aligning
              to the top puts both badges on the same line, which is what makes
              the pair read as a comparison. */}
          <FlexBox
            margin="46px 0 0 0"
            justifyContent="center"
            alignItems="flex-start"
          >
            {/* What these claims do **not** say is the point of the sharpening:
                not that an issuer would never go to a private rail — it gets
                real distribution there — and not that no fintech will ever
                settle on a competitor's ledger, which overstates a real effect
                into something an investor can simply counterexample. The claim
                is **friction**, on two axes: a ledger owned by a competitor is
                an awkward place to settle, and market making on it isn't
                permissionless, so the team most likely to try something new
                meets an account-opening process before it meets any liquidity.

                "Multiple" private ledgers, not "competing" ones — "competing
                private ledgers introduce competitive friction" says the same
                word twice in one breath, and it's the *plurality* that is the
                condition being described. */}
            <VenuePanel
              tint={colors.sell}
              claims={[
                "Multiple private ledgers introduce competitive friction",
                "Liquidity is gated, and market making isn’t permissionless",
                "Early-stage teams face hurdles just to experiment",
              ]}
            >
              {/* No names under these three: each mark is a wordmark that
                  already says the company, so a caption repeats the word
                  directly under itself. Compare the flywheel, whose marks are
                  icons and do need naming. */}
              <LogoRow
                logos={PERMISSIONED}
                width={180}
                height={40}
                tint={colors.sell}
                margin="0px"
                showNames={false}
              />
            </VenuePanel>
            {/* Our side names the **ambition**, not just the contrast: public
                money infrastructure, a flywheel that exists nowhere else, and
                every currency onchain. That is the thing an investor is being
                asked to buy into, and it has to be said here rather than left
                as the implication of the other panel being wrong.

                "…environment **today**" is load-bearing, not filler. It marks
                Solana as the current best choice rather than a permanent
                commitment, which is the answer to the reasonable investor
                question of what happens if a better settlement layer arrives:
                nothing here is bound to this one. Same reason the gap page names
                what Solana is best *at* rather than treating it as given.

                These read as a mechanism rather than as virtues: the environment
                makes transmission and composition cheap, that is what lets
                liquidity compound instead of sitting still, and the compounding
                is what makes every currency reachable — one market at a time,
                which is the honest form of "every currency onchain". A route
                that exists and gets walked, rather than a state we assert. That
                we have already started walking it belongs in the spoken track,
                with the issuers named — a slide saying "we've begun" invites
                "begun with whom?", a question to answer out loud rather than in
                six words under a logo.

                The **absorption** claim is the strongest thing on the page, and
                it is what turns the comparison from a matter of taste into one of
                dominance. A maker who already holds an account on a gated venue
                can quote here and hedge there, so this book can carry that depth
                — and nothing carries it the other way, because no permissioned
                venue can take in a public book's liquidity. Public liquidity is
                therefore a *superset* of private liquidity rather than an
                alternative to it. It also repairs a soft spot elsewhere: page 7
                otherwise leaves our own vaults as the only answer to where depth
                for a thin pair comes from.

                Like the path claim, it is a **capability** and not something
                running today — it depends on how a given maker is integrated.
                The aggressive version (that this is a vampire attack on private
                liquidity, and that it only runs in one direction) is spoken, not
                printed: the page argues about private ledgers as a class rather
                than attacking anyone, and the term properly describes poaching an
                incumbent's liquidity providers with incentives, which is a
                different mechanic. */}
            <VenuePanel
              tint={colors.buy}
              claims={[
                "Dropset is open and composable on Solana, the most money-like onchain environment today",
                "Ease of transmission and open access compound liquidity into a flywheel",
                "Every currency and FX pair has a path to onchain liquidity, one market at a time",
                "Public liquidity absorbs private liquidity through market makers who already have gated access",
              ]}
            >
              <Wordmark width={420} />
            </VenuePanel>
          </FlexBox>
        </SlideBody>
        <Notes>
          The long-term question is whether onchain liquidity is public or
          private, and this is the one to make up your mind about. Arc and Tempo
          are building payment-and-settlement rails, and Canton is doing
          regulated onchain markets — and an issuer that goes there gets real
          distribution, so the argument isn’t that Circle would never use one.
          It’s friction. Once there are several of these ledgers, settling on one
          your competitor owns is an awkward place to be — a bank that competes
          with Circle is not enthusiastic about building on Arc — and market
          making on them isn’t permissionless: the liquidity is gated, so you
          quote only if they let you. An early-stage team hits those hurdles
          before it can even experiment. Dropset is open and composable on
          Solana, the most money-like onchain environment today — and I say today
          deliberately: Solana is where this belongs right now because it’s the
          fastest and the cheapest, not a commitment we’re locked into if
          something better shows up. What we’re building is public liquidity, and
          that’s portable. Ease of transmission and composability let liquidity
          compound into a flywheel instead of sitting still. That’s the public money infrastructure we’re
          building — a flywheel around public currency liquidity that exists
          nowhere else. Every currency and FX pair has a path to onchain
          liquidity here, one market at a time — and we’ve already started
          walking it: we’re in detailed conversations with AUDD, and we’ve spoken
          with the CADC issuer. And here’s the part that matters most: this
          absorbs their liquidity. A market maker who already has an account on
          one of those rails can quote here and hedge there, so their depth shows
          up in a public book that anyone can trade against — and it doesn’t run
          the other way, because a permissioned venue can’t take in a public
          book’s liquidity. Public liquidity is a superset of private liquidity,
          and the makers who already hold that gated access are the pipe.
          [Optional, if the room is technical: it’s a vampire attack on private
          liquidity, and it only works in this direction.] Public liquidity is
          what
          blockchains were built for — moving money is the problem they were
          supposed to solve, and this is that.
        </Notes>
      </Slide>

      {/* 10 — Team & close. The last page, and it stays up. */}
      <Slide>
        <SlideBody>
          <Eyebrow>The team</Eyebrow>
          <Statement fontSize="64px">
            Dropset is built by people who have built exchanges
          </Statement>
          {/* Tighter than the deck's usual 46 for the same reason page 3 is: with
              two five-line bios under two headshots, this is the second densest
              page, and the gap above the portraits is what was pushing the
              eyebrow into the top edge. */}
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
          logistical coordination and partner relationship management. Dropset —
          where currency
          trades onchain. [Leave this page up.]
        </Notes>
      </Slide>
    </Deck>
  );
}
