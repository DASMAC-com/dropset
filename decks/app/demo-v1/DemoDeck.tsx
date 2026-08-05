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
 * Three rules from that spec shape everything below:
 *
 * - **Static images only.** No video and no player: a product beat is an
 *   interface screenshot carrying a claim. Nothing on stage depends on a
 *   network, and every slide prints as a flat page for the accelerator's
 *   combined Google Slides meta-deck.
 * - **No competitor names or logos anywhere.** Partner logos on the growth page
 *   stay — those are companies we're working with, and their marks are the point
 *   — but no threat row and no incumbent row. Those arguments are made in type
 *   here and by name only in the spec's appendix, for conversation.
 * - **Solana is the start, never the ceiling**, and **DASMAC is the company
 *   while Dropset is the protocol**.
 *
 * Nine pages: the title, the gap, the live swap flow, the market data we curate
 * (and its illiquid tail), the eCLOB, how we grow, the growth roadmap, why the
 * open venue wins, and the team — which is last and stays up after the talk, so
 * it's the one page that carries longer copy.
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
 * distinction, which pages 1 and 7 then make explicitly.
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
}: {
  children: React.ReactNode;
  fontSize?: string;
}) => (
  <Heading fontSize={fontSize} margin="0" maxWidth="1540px">
    {children}
  </Heading>
);

/**
 * The three supporting facts on the gap page.
 *
 * These are the deck's one deliberate exception to "no bullet lists": three
 * numbers that are peers, where prose would bury them and the audience is meant
 * to take all three at a glance. They stay **full sentences** so the no-fragment
 * rule still holds, and the marker is a rule rather than a disc — a row of discs
 * is what makes a slide read as a corporate template.
 */
const Fact = ({ children }: { children: React.ReactNode }) => (
  <FlexBox alignItems="flex-start" margin="0 0 20px 0">
    <div
      style={{
        backgroundColor: colors.accent,
        flex: "0 0 auto",
        height: "3px",
        marginRight: "18px",
        marginTop: "18px",
        width: "28px",
      }}
    />
    <div style={{ color: colors.foreground, fontSize: "32px", lineHeight: 1.3 }}>
      {children}
    </div>
  </FlexBox>
);

/**
 * How much of the world's money actually trades on Solana, as a meter.
 *
 * A **meter**, not a pie of two slices: the data is a single ratio against a
 * limit, and the empty part of the track is the whole message. The track is a
 * darker step of the fill's own blue ramp (see `colors.meterTrack`) so the bar
 * reads as one scale rather than two categories, and the count is direct-labeled
 * rather than left to a legend.
 *
 * The screenshot below it isn't decoration — it's the citation. It's our own
 * page showing the same figure, with the URL, so the number is checkable rather
 * than asserted.
 */
const LISTED_CURRENCIES = 14;
const TOTAL_CURRENCIES = 162;
const METER_WIDTH = 760;

const CurrencyMeter = () => (
  <Box width={`${METER_WIDTH}px`}>
    <FlexBox justifyContent="space-between" alignItems="flex-end">
      <div
        style={{
          color: colors.accent,
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "26px",
        }}
      >
        {LISTED_CURRENCIES} on Solana
      </div>
      <div
        style={{
          color: colors.mutedFg,
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "26px",
        }}
      >
        {TOTAL_CURRENCIES} in the world
      </div>
    </FlexBox>
    <div
      style={{
        backgroundColor: colors.meterTrack,
        borderRadius: "5px",
        height: "22px",
        marginTop: "12px",
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
          borderRadius: "0 5px 5px 0",
          height: "100%",
          width: `${(LISTED_CURRENCIES / TOTAL_CURRENCIES) * 100}%`,
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
      <Image src={src} width={width} alt={alt} />
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

/**
 * A step in a left-to-right sequence of captures, with a chevron between steps.
 * Used for the swap flow: it's a beat that used to be a recorded demo, and a
 * numbered sequence of stills is what carries "this happened in order" once the
 * video is gone.
 */
const SequenceArrow = () => (
  <Box margin="0 14px" padding="0 0 40px 0">
    <div style={{ color: colors.accent, fontSize: "44px", lineHeight: 1 }}>›</div>
  </Box>
);

// The caption under a sequence step: what the audience is looking at, in the
// same voice as the spoken beat it stands for. Numbered, so the order is
// explicit rather than merely implied by position.
const StepCaption = ({
  step,
  children,
}: {
  step?: number;
  children: React.ReactNode;
}) => (
  <div
    style={{
      color: colors.mutedFg,
      fontFamily: deckTheme.fonts.monospace,
      fontSize: "22px",
      lineHeight: 1.3,
      marginTop: "12px",
    }}
  >
    {step ? <span style={{ color: colors.accent }}>{step}. </span> : null}
    {children}
  </div>
);

/**
 * A row of captioned logos — the one visual on the page that is about other
 * companies.
 *
 * The marks are each company's own, so they come in two shapes: square icons
 * and logotypes several times as wide as they are tall. Tiles are therefore
 * **height-matched with the width left free** — forcing every mark into one
 * square is what squashed the wide ones flat. Every source here is either
 * transparent or already dark-backed, so tiles carry no fill of their own and
 * the marks sit straight on the deck's black.
 */
type Logo = { name: string; src: string; note: string };

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
  margin = "34px 0 0 0",
}: {
  logos: Logo[];
  width?: number;
  height?: number;
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
        width={`${width + LOGO_CAPTION_ROOM}px`}
      >
        {/* Every tile in a row is the same box; the mark is contained inside it
            rather than sized to it, so a wide logotype and a square icon share a
            footprint without either being stretched. Tile, caption and note all
            sit flush to the column's left edge — centring the tile inside a
            wider column while the text below starts at the column edge left the
            two visibly out of line. */}
        <div
          style={{
            alignItems: "center",
            border: `1px solid ${colors.border}`,
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

// Square: every mark on this page is a pure icon, so `TILE` matches the tile's
// own height (its padding plus the image cap passed below).
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
 * three guesses about revenue; three beats on a timeline read as a plan where
 * each stage funds the next. The dot sits on the rule so the eye takes the order
 * before it reads any of the copy.
 */
type Beat = { when: string; headline: string; body: string };

const BEATS: Beat[] = [
  {
    when: "Now",
    headline: "DASMAC leads the liquidity.",
    body: "We bootstrap the vaults the way Hyperliquid did, and we help issuers get their currency onchain. Dropset is the protocol underneath.",
  },
  {
    when: "Next",
    headline: "Protocol fees accrue value.",
    body: "As we build a mature market, the venue earns on the flow it clears, and the work is getting every pair liquid rather than only the largest.",
  },
  {
    when: "Later",
    headline: "Derivatives are the expansion.",
    body: "Hedging is an extra vertical once spot is nailed, and it is what real market-making operations and mature foreign-exchange markets both run on.",
  },
];

// The full content measure, so the timeline spans the page rather than sitting
// as a narrow band in the middle of it.
const ROADMAP_WIDTH = 1700;
const ROADMAP_COLUMN = 520;

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
 * The open-venue contrast. This is the page that used to be a row of competitor
 * logos, and the rule is that **no competitor is named or shown** — so the
 * argument is made structurally instead: a gated panel beside an open one, each
 * captioned with a full sentence. Nothing here identifies who the gated panel
 * is, which is the intent; the names live in the spec's appendix.
 */
const Contrast = ({
  label,
  sentence,
  tint,
  gated,
}: {
  label: string;
  sentence: string;
  tint: string;
  gated: boolean;
}) => (
  <Box width="620px" margin="0 26px">
    <div
      style={{
        alignItems: "center",
        border: `2px ${gated ? "solid" : "dashed"} ${tint}`,
        borderRadius: "16px",
        boxSizing: "border-box",
        display: "flex",
        height: "180px",
        justifyContent: "center",
        // A gated venue is a filled box you can't see into; an open one is an
        // outline you can. The fill is the entire visual argument, so it tracks
        // `gated` rather than being decoration.
        backgroundColor: gated ? colors.muted : "transparent",
      }}
    >
      <svg
        width="520"
        height="120"
        viewBox="0 0 520 120"
        role="img"
        aria-label={label}
      >
        {gated ? (
          // A wall: participants on both sides of it, none connected through.
          <>
            <rect
              x="248"
              y="6"
              width="8"
              height="108"
              fill={tint}
              opacity="0.9"
            />
            {[40, 100, 160, 360, 420, 480].map((x) => (
              <circle key={x} cx={x} cy="60" r="12" fill={tint} opacity="0.5" />
            ))}
          </>
        ) : (
          // A hub: every participant connected to the same book. Node positions
          // are written out rather than derived — an alternating top/bottom fan
          // is easier to read as six literal points than as an index-parity
          // trick, and it stays obvious which end is which.
          <>
            {[
              { x: 40, y: 18 },
              { x: 130, y: 102 },
              { x: 220, y: 18 },
              { x: 340, y: 102 },
              { x: 430, y: 18 },
              { x: 490, y: 102 },
            ].map(({ x, y }) => (
              <g key={x}>
                <line
                  x1={x}
                  y1={y}
                  x2="260"
                  y2="60"
                  stroke={tint}
                  strokeWidth="2"
                  opacity="0.45"
                />
                <circle cx={x} cy={y} r="10" fill={tint} opacity="0.6" />
              </g>
            ))}
            <circle cx="260" cy="60" r="18" fill={tint} />
          </>
        )}
      </svg>
    </div>
    <div
      style={{
        color: colors.foreground,
        fontSize: "26px",
        lineHeight: 1.35,
        marginTop: "18px",
      }}
    >
      {sentence}
    </div>
  </Box>
);

/**
 * Team headshots, mirrored from the marketing site at build time, captioned.
 * Left square and unframed — the sources are already square, and both photos
 * are shot on a dark background that reads as part of the slide.
 *
 * Page 9 is the last page and stays on screen after the talk ends, so it can
 * carry more than a line each — but only just. The bios are deliberately one
 * sentence: an earlier draft argued *why* each role mattered, which read as
 * justifying the team rather than stating what they've done.
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
          <FlexBox margin="42px 0 0 0" alignItems="flex-start">
            <Box width="740px" margin="0 60px 0 0">
              <Fact>It trades over $9 trillion a day.</Fact>
              <Fact>
                Its liquidity is fragmented across obfuscated
                over-the-counter desks.
              </Fact>
              <Fact>
                Less than 10% of the world’s currencies are available on Solana
                today.
              </Fact>
            </Box>
            <Box>
              <CurrencyMeter />
              <Screenshot
                src="/screens/currencies-listed.png"
                width={METER_WIDTH}
                alt="14 of 162 currencies represented on Solana; 148 not yet listed"
                source="dropset.io/currencies"
                margin="26px 0 0 0"
              />
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          Foreign exchange is the biggest market on earth — over nine trillion
          dollars a day. But it only trades 24/5, its liquidity is fragmented
          across obfuscated over-the-counter desks, and less than ten percent of
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
          <Statement fontSize="64px">
            Dropset settles real currency trades on mainnet today.
          </Statement>
          {/* The swap flow as three stills — the beat that used to be the
              mainnet recording. Picker and search stack in one column because
              both are short and sequential; the settled trade gets its own
              column because that capture is tall and is the payoff. */}
          <FlexBox margin="34px 0 0 0" justifyContent="center" alignItems="center">
            <Box>
              <Screenshot
                src="/screens/swap-picker.png"
                width={340}
                alt="The swap panel, choosing the currency to receive"
                margin="0"
              />
              <StepCaption step={1}>Open the currency picker</StepCaption>
              <Screenshot
                src="/screens/swap-search.png"
                width={340}
                alt="Searching for a currency and its available stablecoins"
                margin="20px 0 0 0"
              />
              <StepCaption step={2}>Type the currency you want</StepCaption>
            </Box>
            <SequenceArrow />
            <Box>
              <Screenshot
                src="/screens/swap-settled.png"
                width={340}
                alt="A priced USDC to EURC swap, with the route drawn on the globe"
                margin="0"
              />
              <StepCaption step={3}>Swap, and it settles</StepCaption>
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          This already works. Dropset is live on mainnet today, clearing real
          trades: you open the picker, type the currency you want, and swap.
          Settlement is atomic, the ramps are near instant, and the venue never
          closes. Solana is the start, not the end — it’s the most
          moneyness-conducive environment onchain. [Today we clear by routing
          through aggregators and sourcing existing liquidity; don’t claim “most
          liquid”.]
        </Notes>
      </Slide>

      {/* 4 — The market data we curate, and its illiquid tail */}
      <Slide>
        <SlideBody>
          <Eyebrow>Every currency, in one place</Eyebrow>
          <Statement fontSize="64px">
            We curate the market data for every currency onchain — and the long
            tail has no liquidity at all.
          </Statement>
          <FlexBox margin="30px 0 0 0" justifyContent="center" alignItems="flex-start">
            <Box margin="0 20px 0 0">
              <Screenshot
                src="/screens/currencies-by-country.png"
                width={560}
                alt="Currencies grouped by country, with price, volume and liquidity"
                caption="Grouped by country"
                margin="0"
              />
            </Box>
            <Box margin="0 0 0 20px">
              <Screenshot
                src="/screens/currencies-by-liquidity.png"
                width={560}
                alt="The same currencies sorted by on-chain liquidity, deepest first"
                margin="0"
              />
              <Screenshot
                src="/screens/currencies-illiquid.png"
                width={560}
                alt="The tail of the same table: eleven currencies with no liquidity, volume or price"
                caption="Sorted by liquidity — and the tail is empty"
                margin="16px 0 0 0"
              />
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          Dropset already settles trades on mainnet by sourcing existing
          liquidity from the other venues — and alongside that we curate the
          market data for every currency onchain: price, volume, market cap,
          liquidity, holders, grouped by country or sorted however you want. Sort
          by liquidity and the story tells itself. A handful of pairs are deep,
          and then the long tail is completely dry — the Australian dollar, the
          Canadian dollar, the yen, the naira, the lira, all sitting there with
          no market at all. Those are the currencies we’re here to make liquid,
          and that needs a venue.
        </Notes>
      </Slide>

      {/* 5 — The eCLOB */}
      <Slide>
        <SlideBody>
          <Eyebrow>The eCLOB</Eyebrow>
          <Statement fontSize="64px">
            Our design gives order-book transparency with propAMM efficiency.
          </Statement>
          {/* Left column stacks the two proof captures (the maker's own control
              panel, and what a quote update costs); the right column is the
              payoff — the same market rendered on the frontend, book and trades
              tape and a filled order together. */}
          <FlexBox margin="30px 0 0 0" justifyContent="center" alignItems="flex-start">
            <Box margin="0 22px 0 0">
              <Screenshot
                src="/screens/maker-tui.png"
                width={400}
                alt="The maker control panel: seven FX markets and a live book"
                caption="Market maker TUI"
                margin="0"
              />
              <Screenshot
                src="/screens/compute-units.png"
                width={400}
                alt="Compute units per instruction: a reprice costs 47, a reshape 59"
                caption="Reprice: 47 CU · reshape: 59 CU"
                margin="20px 0 0 0"
              />
            </Box>
            <Box margin="0 0 0 22px">
              <Screenshot
                src="/screens/eclob-frontend.png"
                width={640}
                alt="The eCLOB on the frontend: a EURC/USDC order book, a live trades tape, and a settled order"
                caption="The same market on the frontend: book, trades, and a filled order"
                margin="0"
              />
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          Making a market onchain used to be prohibitively expensive — gas made
          continuous quoting impossible, so everything before this was a
          band-aid. We’ve built order books before, so we built one that fits:
          the eCLOB gives you the transparency of a central limit order book with
          quote updates as cheap as a propAMM. Repricing the whole book costs
          forty-seven compute units and reshaping the ladder fifty-nine, on a
          chain that gives you two hundred thousand per instruction. On the left
          is our own maker running seven markets; on the right is that market on
          the frontend, with the book, the live trades tape, and a filled order.
          We’re building this out so anyone can quote onchain with a vault-style
          approach.
        </Notes>
      </Slide>

      {/* 6 — How we grow */}
      <Slide>
        <SlideBody>
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="68px">
            Our vaults bootstrap a public FX liquidity flywheel.
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

      {/* 7 — Growth roadmap */}
      <Slide>
        <SlideBody>
          <Eyebrow>Growth roadmap</Eyebrow>
          <Statement fontSize="68px">
            Each stage of the roadmap funds the next one.
          </Statement>
          <Roadmap />
        </SlideBody>
        <Notes>
          Today DASMAC leads the liquidity: we bootstrap the vaults the way
          Hyperliquid did, and we help issuers get their currency onchain —
          DASMAC is the company, Dropset is the protocol it runs on. Next, as we
          build a mature market, protocol fees accrue value to the venue, and the
          job is getting every pair liquid rather than just the biggest ones.
          Later, derivatives are the expansion: hedging is an extra vertical once
          we’ve nailed spot, and it’s what serious market-making operations and
          mature FX markets both run on.
        </Notes>
      </Slide>

      {/* 8 — Why the open venue wins */}
      <Slide>
        <SlideBody>
          <Eyebrow>Why the open venue wins</Eyebrow>
          <Statement fontSize="60px">
            The people who actually need foreign exchange need an open system,
            and permissioned liquidity is not public.
          </Statement>
          {/* No names and no marks on this page — the contrast is structural.
              See the `Contrast` note: naming a competitor on-slide hands them
              the frame, and the argument survives without it. */}
          <FlexBox margin="44px 0 0 0" justifyContent="center">
            <Contrast
              label="A gated venue: participants either side of a wall, none connected through it"
              sentence="Permissioned liquidity sits behind a gate, and you cannot make a market unless they let you in."
              tint={colors.sell}
              gated
            />
            <Contrast
              label="An open venue: every participant connected to one book"
              sentence="Dropset is open, neutral, and composable, so anyone can quote, anyone can trade, and any app can integrate."
              tint={colors.buy}
              gated={false}
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          FX’s end consumers need an open system. The honest risk is that
          whoever owns distribution permissions onchain settlement — and some of
          them will try. But permissioned liquidity isn’t public: you can’t make
          a market unless they let you. Dropset is open, neutral, and composable
          — anyone can quote, anyone can trade, any app can integrate. And the
          venues that are public are built for a different customer; they’re
          serving day traders, not the business that needs to settle an invoice
          in another currency. That’s also why we started on Solana: it’s the
          most moneyness-conducive environment onchain. [Names stay off the slide
          and out of the talk — the appendix has them if a question goes there.]
        </Notes>
      </Slide>

      {/* 9 — Team & close. The last page, and it stays up. */}
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
              bio="Authored two exchanges on Aptos, including the Econia order book, which settled around $500M — and wrote the Solana Opcode Guide."
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy Sosa"
              role="Operations, DASMAC"
              prior="prev. EA, Dragonfly Capital"
              bio="Owns the whole operational stack, working with the banks, stablecoin providers, onramps and service providers we build on."
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          Dropset is built by people who have built exchanges. I authored two on
          Aptos, including the Econia order book, which settled around five
          hundred million in volume, and I wrote the Solana Opcode Guide — the
          playbook for squeezing performance out of Solana programs, which is
          what makes quoting on the eCLOB cost double-digit compute units. Judy
          owns the whole operational stack, and works directly with the banks,
          the stablecoin providers, the onramps and the service providers we
          build on. Dropset — where currency trades onchain. [Leave this page up.]
        </Notes>
      </Slide>
    </Deck>
  );
}
