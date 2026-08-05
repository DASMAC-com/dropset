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
 * - **Solana is the start, never the ceiling**, and **DASMAC is the company
 *   while Dropset is the protocol**.
 *
 * Ten pages, and the middle five are one argument in sequence: the swap flow
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
 * Persistent footer: wordmark on the left, the DASMAC credit in the middle,
 * progress dots on the right. The DASMAC mark is a transparent PNG, so unlike
 * the Dropset one it needs no blend to sit on the dark backdrop.
 *
 * The credit reads **"Built by DASMAC"**, not "Courtesy of" — authorship, not a
 * loan. It's also the smallest place the deck makes its company/protocol
 * distinction, which pages 1 and 8 then make explicitly.
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
    <FlexBox alignItems="center">
      <Text color="quaternary" fontSize="22px" margin="0 14px 0 0">
        Built by
      </Text>
      <img src="/dasmac-wordmark.png" alt="DASMAC" width={110} />
    </FlexBox>
    <Box padding="0 1.25em">
      <Progress color={colors.accent} size={11} />
    </Box>
  </FlexBox>
);

/**
 * Every slide's content column. The bottom padding is the point: slide content
 * is centred in the full slide, and the footer sits on top of that same space,
 * so without it the lowest element on a busy page crowds the DASMAC credit.
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
    padding="0 0 106px 0"
  >
    {children}
  </FlexBox>
);

/**
 * Small monospace kicker that labels each content slide. Uppercase, letterspaced
 * Space Mono — which is the exact treatment the DASMAC company banner uses for
 * its own tag ("DISTRIBUTED ATOMIC STATE MACHINE ALGORITHMS CORPORATION"), so
 * the deck's kickers and the brand art are visibly the same system. Sentence
 * case stays with Inter, where it belongs.
 */
const Eyebrow = ({ children }: { children: React.ReactNode }) => (
  <Text
    color="secondary"
    fontFamily="monospace"
    fontSize="26px"
    margin="0 0 14px 0"
    style={{ letterSpacing: "0.14em", textTransform: "uppercase" }}
  >
    {children}
  </Text>
);

/**
 * The sentence a page is built around. Sized down from the theme's `h1` and
 * width-capped so a whole sentence lands in one or two lines at a readable
 * measure rather than spanning the slide.
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
    margin="0"
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
 * The three supporting facts on the gap page.
 *
 * `justifyContent` is the load-bearing prop here, and it must stay explicit:
 * Spectacle's `FlexBox` **defaults to `center`**, which centred each row
 * independently — so the short one-line fact sat visibly inset from the wrapped
 * two-line ones, reading as a stray indent rather than a list.
 *
 * The marker is a chevron rather than a disc: a row of discs is what makes a
 * slide read as a corporate template. It's the angular "greater-than" shape the
 * review asked for, but deliberately *not* a literal `≥` — that glyph makes a
 * numeric claim, and it would flatly contradict the third fact, which is a
 * *less-than*.
 */
const Fact = ({ children }: { children: React.ReactNode }) => (
  <FlexBox
    alignItems="flex-start"
    justifyContent="flex-start"
    margin="0 0 26px 0"
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
    <div style={{ color: colors.foreground, fontSize: "34px", lineHeight: 1.25 }}>
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
  <Box width={`${METER_WIDTH}px`}>
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
      border={`1px solid ${colors.border}`}
      borderRadius="12px"
      padding="14px 18px"
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
    {caption ? (
      <Text color="quaternary" fontSize="24px" margin="10px 0 0 0">
        {caption}
      </Text>
    ) : null}
  </Box>
);

// The chevron between steps of the swap flow.
const SequenceArrow = () => (
  <Box margin="0 16px">
    <div style={{ color: colors.accent, fontSize: "44px", lineHeight: 1 }}>›</div>
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
 * statement is pinned to one line by its own `maxWidth`, and the URL moved into
 * the middle column. Those are load-bearing — undo either and this has to come
 * back down.
 */
const STEP_WIDTH = 420;

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
  /** Omit to label a stage that isn't a numbered user action — see page 6. */
  step?: number;
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
      {step ? <span style={{ color: colors.accent }}>{step}. </span> : null}
      {label}
    </div>
    <Box
      border={`1px solid ${colors.border}`}
      borderRadius="12px"
      padding="12px 14px"
      backgroundColor={colors.muted}
    >
      <Image
        src={src}
        width={width - 28}
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
            the last line into the footer. */}
        {note ? (
          <>
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
          </>
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
 * Captions carry the name the presenter says, which isn't always what the mark
 * says: Arc is Circle's, so Circle's logo is what an audience recognizes.
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
  {
    when: "Now",
    headline: "DASMAC bootstraps liquidity.",
    body: "DASMAC bootstraps nascent FX pairs by leading Hyperliquid-style vaults using the Dropset protocol.",
  },
  {
    when: "Next",
    headline: "Protocol fees accrue value.",
    body: "As markets mature, volume and fees compound, and currency pairs achieve deep liquidity.",
  },
  {
    when: "Later",
    headline: "Derivatives provide an expansion opportunity.",
    body: "Once spot is fully mature, hedging instruments and additional derivatives enable more efficient market making and more mature markets.",
  },
];

// The full content measure, so the timeline spans the page rather than sitting
// as a narrow band in the middle of it.
const ROADMAP_WIDTH = 1780;
const ROADMAP_COLUMN = 540;

const Roadmap = () => (
  <Box margin="46px 0 0 0" width={`${ROADMAP_WIDTH}px`}>
    {/* The rule and its dots are one row above the columns rather than a border
        on each column: a per-column border restarts at every gap, and the
        unbroken line is what makes the three beats read as one sequence. */}
    <FlexBox justifyContent="space-between" alignItems="center">
      {BEATS.map(({ when }, index) => (
        <FlexBox key={when} alignItems="center" width="100%">
          <div
            style={{
              backgroundColor: colors.accent,
              borderRadius: "50%",
              flex: "0 0 auto",
              height: "18px",
              width: "18px",
            }}
          />
          <div
            style={{
              backgroundColor:
                index === BEATS.length - 1 ? "transparent" : colors.border,
              height: "2px",
              width: "100%",
            }}
          />
        </FlexBox>
      ))}
    </FlexBox>
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
 * One side of the open-venue comparison: a bordered panel with a caption under
 * it. The border color is the whole argument — the permissioned side is tinted
 * with the sell red, the Dropset side with the buy green — so the page reads
 * before any of its copy does.
 */
const VenuePanel = ({
  tint,
  caption,
  children,
}: {
  tint: string;
  caption: React.ReactNode;
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
    <div
      style={{
        color: colors.foreground,
        fontSize: "27px",
        lineHeight: 1.35,
        marginTop: "20px",
      }}
    >
      {caption}
    </div>
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
    <Text fontSize="38px" margin="18px 0 0 0">
      {name}
    </Text>
    <Text fontSize="28px" margin="6px 0 0 0">
      {role}
    </Text>
    <Text
      color="secondary"
      fontFamily="monospace"
      fontSize="23px"
      margin="8px 0 0 0"
    >
      {prior}
    </Text>
    <Text color="quaternary" fontSize="26px" margin="16px 0 0 0">
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
          <Statement fontSize="88px">Where currency trades onchain.</Statement>
          {/* "Built by DASMAC" is on-slide as well as in the footer: the title
              is where the company/protocol distinction has to land first, and
              the footer credit is too small to carry it alone. */}
          <Text color="quaternary" fontSize="40px" margin="22px 0 0 0">
            Built by DASMAC.
          </Text>
          {/* The company banner, uncaptioned. It's brand art, not a figure —
              a caption explaining what a banner is would undercut it. */}
          <Box margin="52px 0 0 0">
            <img
              src="/dasmac-banner-wide.png"
              alt="DASMAC — Distributed Atomic State Machine Algorithms Corporation"
              width={980}
              style={{ borderRadius: "10px", display: "block" }}
            />
          </Box>
        </SlideBody>
        <Notes>
          Dropset is where currency trades onchain, built by DASMAC.
        </Notes>
      </Slide>

      {/* 2 — The gap */}
      <Slide>
        <SlideBody>
          <Eyebrow>The gap</Eyebrow>
          <Statement fontSize="72px">
            Foreign exchange is the biggest market on earth.
          </Statement>
          <FlexBox
            margin="44px 0 0 0"
            alignItems="flex-start"
            justifyContent="flex-start"
          >
            <Box width="800px" margin="0 60px 0 0">
              <Fact>Over $9 trillion daily volume</Fact>
              <Fact>Liquidity is fragmented across OTC and banks</Fact>
              <Fact>
                Less than 10% of the world’s currencies are available on Solana
              </Fact>
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
          dollars a day. But it only trades 24/5, its liquidity is fragmented
          across over-the-counter desks and banks, and less than ten percent of
          the world’s currencies are even available on Solana today: fourteen out
          of a hundred and sixty-two, and that count is live on our own site,
          which is where this is from. Every currency should be connectable to
          every other one, and that’s what we’re building. To be precise: we
          don’t issue currencies — issuers create them, and Dropset is where they
          trade.
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
            Dropset already processes Solana mainnet FX trades.
          </Statement>
          {/* The swap flow left to right, one step per column — the beat that
              used to be the mainnet recording. The three captures are very
              different heights, and centring them vertically is what makes that
              read: each caption sits directly above its own capture, so the
              labels climb like steps as the captures grow taller. The chevrons
              inherit the same centring, landing on the row's axis rather than
              pinned to the top of the tallest column. */}
          <FlexBox margin="36px 0 0 0" justifyContent="center" alignItems="center">
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
            Dropset curates market data for all Solana-based currencies.
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
            Many currencies have no liquidity whatsoever.
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
          {/* One sentence, not a statement plus a supporting line. It names the
              eCLOB even though the eyebrow above already does — the kicker is a
              small tag and the sentence is the claim, and the name is worth
              landing twice on the one page that introduces it. The compute-unit
              numbers live on the capture that shows them rather than being
              restated here. */}
          {/* Pinned to one line by `nowrap`, not by a width estimate. Every
              earlier attempt at this heading wrapped further than intended, and
              the page's own overflow then clipped this slide's eyebrow off the
              top — three review rounds running. Short copy plus `nowrap` ends
              that: the browser now enforces what the budget assumes. */}
          <Statement fontSize="56px" nowrap>
            Dropset ships propAMM efficiency and order-book transparency.
          </Statement>
          {/* Left to right as a pipeline, not a stack: what a quote costs, then
              the maker paying that cost to quote a live market, then the same
              liquidity arriving on the frontend. It reads low-level → system →
              product, which is the actual story, and three columns of one
              capture each are far shorter than two stacked in a column — the
              headroom that finally puts this page comfortably inside its slide.
              Captions sit above on a shared baseline, as on page 3, because the
              three captures are very different heights. */}
          <FlexBox
            margin="36px 0 0 0"
            justifyContent="center"
            alignItems="flex-start"
          >
            <Step
              label="Quoting costs 47 CU"
              src="/screens/compute-units.png"
              width={500}
              alt="Compute units per instruction: a reprice costs 47, a reshape 59"
            />
            <SequenceArrow />
            <Step
              label="Demo maker quoting locally"
              src="/screens/maker-tui.png"
              width={500}
              alt="The maker control panel: seven FX markets and a live book"
            />
            <SequenceArrow />
            <Step
              label="Liquidity routes to the frontend"
              src="/screens/eclob-frontend.png"
              width={500}
              alt="The eCLOB on the frontend: a EURC/USDC order book, a live trades tape, and a settled order"
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          So we’re building the exchange those markets need. Making a market
          onchain used to be prohibitively expensive — gas made continuous
          quoting impossible, so everything before this was a band-aid. We’ve
          built order books before, so we built one that fits: our exchange model
          gives you the transparency of a central limit order book with quote
          updates as cheap as a propAMM. Repricing the whole book costs forty-seven
          compute units and reshaping the ladder fifty-nine, on a chain that
          gives you two hundred thousand per instruction. On the left is our own
          maker quoting a market locally; on the right that same liquidity routed
          through to the frontend, with the book, the live trades tape, and a
          filled order. We’re building this out so anyone can quote onchain with
          a vault-style approach.
        </Notes>
      </Slide>

      {/* 7 — How we grow */}
      <Slide>
        <SlideBody>
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="68px">
            FX vaults bootstrap a public liquidity flywheel.
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
          <Statement fontSize="68px">
            A deliberate and methodical path to expansion.
          </Statement>
          <Roadmap />
        </SlideBody>
        <Notes>
          Now, DASMAC bootstraps the liquidity: we lead Hyperliquid-style vaults
          on nascent FX pairs, using the Dropset protocol — DASMAC the company,
          Dropset the protocol. Next, protocol fees accrue value: as markets
          mature, volume and fees compound, and the currency pairs achieve deep
          liquidity. Later, derivatives provide an expansion
          opportunity: once spot is fully mature, hedging instruments and
          additional derivatives enable more efficient market making and more
          mature markets.
        </Notes>
      </Slide>

      {/* 9 — Why the open venue wins */}
      <Slide>
        <SlideBody>
          <Eyebrow>Why the open venue wins</Eyebrow>
          <Statement fontSize="64px">
            Permissioned onchain liquidity has an adoption ceiling.
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
            <VenuePanel
              tint={colors.sell}
              caption="Permissioned solutions are blocking composability. Competitive dynamics prevent fintech companies from adopting a competitor’s private ledger."
            >
              <LogoRow
                logos={PERMISSIONED}
                width={180}
                height={40}
                tint={colors.sell}
                margin="0"
              />
            </VenuePanel>
            <VenuePanel
              tint={colors.buy}
              caption="Dropset is open and composable on Solana, the most money-like onchain environment, where ease of transmission and composability are maximized. Public liquidity is what blockchains were built for."
            >
              <Wordmark width={420} />
            </VenuePanel>
          </FlexBox>
        </SlideBody>
        <Notes>
          Permissioned onchain liquidity has an adoption ceiling. Arc and Tempo
          are building payment-and-settlement rails, and Canton is doing
          regulated onchain markets — any of them could decide FX is theirs, and
          each arrives with the customers already on it. But their liquidity
          isn’t public: you can’t make a market unless they let you, and that
          blocks composability for everyone downstream. And competitive dynamics
          stop it before it starts: a fintech isn’t going to settle on a
          competitor’s private ledger. A bank that competes with Circle won’t
          build on Arc, and a multi-signature banking product isn’t going to run
          on Canton. Dropset is open, neutral and composable: anyone can quote,
          anyone can trade, any app can integrate. Public liquidity is what
          blockchains were built for — moving money is the problem they were
          supposed to solve, and this is that.
          And that’s why we started on Solana: the most money-like onchain
          environment there is, where ease of transmission and composability are
          both maximized.
        </Notes>
      </Slide>

      {/* 10 — Team & close. The last page, and it stays up. */}
      <Slide>
        <SlideBody>
          <Eyebrow>The team</Eyebrow>
          <Statement fontSize="64px">
            Dropset is built by people who have built exchanges.
          </Statement>
          <FlexBox
            margin="46px 0 0 0"
            justifyContent="center"
            alignItems="flex-start"
          >
            <Portrait
              src="/remote/team-alex.png"
              name="Alex Kahn"
              role="Founder, DASMAC"
              prior="prev. Cofounder, Econia Labs"
              bio="Authored two exchanges on Aptos, including the Econia order book ($500M lifetime volume). Authored the Solana Opcode Guide, the definitive resource for optimizing Solana program efficiency."
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy Sosa"
              role="Operations, DASMAC"
              prior="prev. EA, Dragonfly Capital"
              bio="Owns the whole operational stack, working with banks, stablecoin providers, onramps and service providers."
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          Dropset is built by people who have built exchanges. I authored two on
          Aptos, including the Econia order book, five hundred million dollars of
          lifetime volume, and I authored the Solana Opcode Guide, the definitive
          resource for optimizing Solana program efficiency — which is what makes
          quoting on the eCLOB cost double-digit compute units. Judy owns the
          whole operational stack, and works directly with banks, stablecoin
          providers, onramps and service providers. Dropset — where currency
          trades onchain. [Leave this page up.]
        </Notes>
      </Slide>
    </Deck>
  );
}
