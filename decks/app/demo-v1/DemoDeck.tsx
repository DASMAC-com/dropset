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
 * The demo-day pitch deck — a ~115-second accelerator pitch. Slides are
 * backdrops the presenter talks over; the full spoken script lives in each
 * slide's `<Notes>` (presenter mode, `⌘⇧P` — a bare `p` does nothing), never on
 * the slide itself. The route name is public-facing (`/demo-v1`); internal
 * ticket ids never appear here or in the URL.
 *
 * The copy follows `../../demo-v1-spec.md`, which is the source of truth for
 * it. Edit the spec first, then the deck.
 *
 * This is **outline v2**, reworked from round-1 reviewer feedback. Three of its
 * rules shape everything below:
 *
 * - **Static images only.** There is no video and no player: the recorded demos
 *   and their click-to-play badge are gone, and a product beat is an interface
 *   screenshot carrying a full-sentence claim. Nothing on stage depends on a
 *   network, and every slide prints as a flat page.
 * - **Full sentences on-slide**, not fragment headlines — a reviewer scrolling
 *   the deck without the talk should still get the argument.
 * - **No competitor names or logos anywhere.** The threats row and the
 *   incumbents row are both retired; the arguments they carried are made in
 *   type here and by name only in the spec's appendix, where they can be
 *   answered in conversation rather than displayed.
 *
 * Eight pages: the title, the gap, what's live today, the eCLOB, how we grow,
 * commercial viability, why the open venue wins, and the team — which is the
 * last page and stays up after the talk, so it's the one that carries long
 * copy.
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
 * distinction, which pages 1 and 6 then make explicitly.
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

// Small monospace kicker that labels each content slide. Space Mono, per the
// brand — it's the tag face across the product site.
const Eyebrow = ({ children }: { children: React.ReactNode }) => (
  <Text
    color="secondary"
    fontFamily="monospace"
    fontSize="30px"
    margin="0 0 12px 0"
  >
    {children}
  </Text>
);

/**
 * The sentence a page is built around. Full sentences are a v2 rule, so this is
 * sized down from the theme's `h1` and width-capped: a whole sentence has to
 * land in one or two lines at a readable measure rather than spanning the slide.
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
 * A secondary full sentence under a `Statement` — the supporting clause on the
 * pages whose argument genuinely needs two beats (the open-venue page's risk
 * and answer). Deliberately not a bullet: v2 bans lists, and two sentences at
 * different weights read as prose where two bullets read as a list.
 */
const Supporting = ({
  children,
  color = "quaternary",
  width = "1100px",
}: {
  children: React.ReactNode;
  color?: string;
  width?: string;
}) => (
  <Text color={color} fontSize="32px" margin="22px 0 0 0" maxWidth={width}>
    {children}
  </Text>
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
}: {
  src: string;
  width: number;
  alt: string;
  source?: string;
  caption?: string;
}) => (
  <Box margin="34px 0 0 0">
    <Box
      border={`1px solid ${colors.border}`}
      borderRadius="12px"
      padding="16px 22px"
      backgroundColor={colors.muted}
    >
      <Image src={src} width={width} alt={alt} />
    </Box>
    {source ? (
      <Link href={`https://${source}`} fontSize="26px" margin="10px 0 0 0">
        {source}
      </Link>
    ) : null}
    {caption ? (
      <Text color="quaternary" fontSize="26px" margin="10px 0 0 0">
        {caption}
      </Text>
    ) : null}
  </Box>
);

/**
 * A labeled stand-in for art that doesn't exist yet — the two brand banners and
 * the mainnet capture sequence. Deliberately **loud** rather than tasteful: a
 * dashed frame that says what belongs in it can't be mistaken for a design
 * choice, so nothing ships to a projector with a quiet empty box on it.
 *
 * Every frame states its own intended pixel shape, because these are the boxes
 * the real captures have to fit — a placeholder that doesn't hold the eventual
 * aspect ratio just moves the layout work to the day the assets arrive.
 */
const Placeholder = ({
  label,
  width,
  height,
  note,
}: {
  label: string;
  width: number;
  height: number;
  note?: string;
}) => (
  <Box>
    <div
      style={{
        alignItems: "center",
        border: `2px dashed ${colors.accent}`,
        borderRadius: "12px",
        boxSizing: "border-box",
        display: "flex",
        flexDirection: "column",
        gap: "8px",
        height: `${height}px`,
        justifyContent: "center",
        opacity: 0.75,
        padding: "0 24px",
        textAlign: "center",
        width: `${width}px`,
      }}
    >
      <div
        style={{
          color: colors.accent,
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "22px",
          lineHeight: 1.3,
        }}
      >
        {label}
      </div>
      <div
        style={{
          color: colors.mutedFg,
          fontFamily: deckTheme.fonts.monospace,
          fontSize: "18px",
        }}
      >
        {width}×{height}
      </div>
    </div>
    {note ? (
      <div
        style={{
          color: colors.mutedFg,
          fontSize: "22px",
          lineHeight: 1.3,
          marginTop: "10px",
          maxWidth: `${width}px`,
        }}
      >
        {note}
      </div>
    ) : null}
  </Box>
);

/**
 * The brand-banner pair, on the title page. The DASMAC company banner (the
 * Twitter one, with the mountains) isn't in `brand-assets/`, so both halves
 * ship as placeholders — and showing them *as a pair* is the point: the
 * proposal being made here is the lockup, company banner beside protocol
 * banner, which is easier to judge as two empty frames in position than as
 * prose. Sized 3:1, the shape a Twitter banner already is.
 */
const BannerPair = () => (
  <FlexBox margin="46px 0 0 0" justifyContent="center" alignItems="flex-start">
    <Box margin="0 22px">
      <Placeholder
        label="DASMAC company banner"
        width={640}
        height={214}
        note="The company banner — the one with the mountains."
      />
    </Box>
    <Box margin="0 22px">
      <Placeholder
        label="Dropset protocol banner"
        width={640}
        height={214}
        note="Its protocol counterpart, to be made to match."
      />
    </Box>
  </FlexBox>
);

/**
 * A step in a left-to-right sequence of captures, with a chevron between steps.
 * Used for the mainnet swap flow (page 3) and the book-bootstrap strip (page
 * 4): both are beats that used to be a video, and a sequence of stills is what
 * carries "this happened in order" once the video is gone.
 */
const SequenceArrow = () => (
  <Box margin="0 18px" padding="0 0 40px 0">
    <div style={{ color: colors.accent, fontSize: "40px", lineHeight: 1 }}>
      ›
    </div>
  </Box>
);

const Sequence = ({ children }: { children: React.ReactNode[] }) => (
  <FlexBox margin="34px 0 0 0" justifyContent="center" alignItems="center">
    {children.map((step, index) => (
      // Index keys are correct here: a sequence is a fixed, ordered literal
      // written out in the page below, never reordered or filtered at runtime.
      <FlexBox key={index} alignItems="center">
        {index > 0 ? <SequenceArrow /> : null}
        {step}
      </FlexBox>
    ))}
  </FlexBox>
);

// The caption under a sequence step: what the audience is looking at, in the
// same voice as the spoken beat it stands for.
const StepCaption = ({ children }: { children: React.ReactNode }) => (
  <div
    style={{
      color: colors.mutedFg,
      fontFamily: deckTheme.fonts.monospace,
      fontSize: "22px",
      lineHeight: 1.3,
      marginTop: "12px",
      textAlign: "center",
    }}
  >
    {children}
  </div>
);

/**
 * A partner on the growth page. **Text-only, no logo** — a v2 rule, and the fix
 * for the round-1 "why are these companies here?" confusion: a logo grid never
 * said what the relationship was, and the sentence does. The logo outreach that
 * would have supplied real marks is dropped, so there is no asset to wait on.
 */
type Partner = { name: string; relationship: string };

const PartnerTile = ({ name, relationship }: Partner) => (
  <Box
    width="360px"
    margin="0 14px"
    padding="20px 22px"
    border={`1px solid ${colors.border}`}
    borderRadius="14px"
  >
    <div
      style={{
        color: colors.foreground,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "26px",
        lineHeight: 1.2,
      }}
    >
      {name}
    </div>
    {/* A plain element, not `Text`: Spectacle's carries a theme margin of its
        own that opens a gap far larger than either margin asks for, which on a
        four-tile row pushes the last line into the footer. */}
    <div
      style={{
        color: colors.mutedFg,
        fontSize: "22px",
        lineHeight: 1.35,
        marginTop: "10px",
      }}
    >
      {relationship}
    </div>
  </Box>
);

/**
 * The two ends of the flywheel. Issuers sit **upstream**: they mint the
 * currency and need it to trade. Payments companies sit **downstream**: they
 * consume the liquidity to settle real invoices. Naming both directions is the
 * point of the page — a venue needs each end to bootstrap.
 *
 * Every relationship line says both what the company is to us *and* that we've
 * actually spoken with them, since "we know these people" is the claim the
 * page is really making.
 */
const UPSTREAM: Partner[] = [
  {
    name: "AUDD Digital",
    relationship:
      "Issues the Australian dollar onchain, and we have talked with them about sourcing its liquidity.",
  },
  {
    name: "Loon",
    relationship:
      "Issues CADC, the Canadian dollar, and is in conversation with us about a market for it.",
  },
];

const DOWNSTREAM: Partner[] = [
  {
    name: "Altitude",
    relationship:
      "Banking infrastructure that has to buy foreign exchange to move client money.",
  },
  {
    name: "CargoBill",
    relationship:
      "Settles cross-border supply-chain invoices, which is FX demand arriving every day.",
  },
];

/**
 * One end of the flywheel: a heading with a rule under it, spanning both of
 * that end's tiles. Boxing each end in a filled panel did make the split
 * obvious, but at the cost of two slabs that dominated the page — the
 * underline says "these two belong together" with none of that weight.
 */
// A group is its two tiles plus their margins, so its heading rule spans
// exactly its own pair. Derived rather than repeated, so widening a tile can't
// silently leave the rule matching a width nothing has any more.
const PARTNER_TILE_WIDTH = 360;
const PARTNER_TILE_MARGIN = 14;
const FLYWHEEL_GROUP = 2 * (PARTNER_TILE_WIDTH + 2 * PARTNER_TILE_MARGIN);

const FlywheelEnd = ({
  label,
  partners,
}: {
  label: string;
  partners: Partner[];
}) => (
  <Box width={`${FLYWHEEL_GROUP}px`}>
    <div
      style={{
        color: colors.accent,
        fontFamily: deckTheme.fonts.monospace,
        fontSize: "26px",
        lineHeight: 1.2,
        marginBottom: "12px",
      }}
    >
      {label}
    </div>
    <div
      style={{ backgroundColor: colors.accent, height: "2px", width: "100%" }}
    />
    <FlexBox margin="22px 0 0 0" alignItems="flex-start">
      {partners.map((partner) => (
        <PartnerTile key={partner.name} {...partner} />
      ))}
    </FlexBox>
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
  <Box margin="26px 0 0 0" width={`${2 * FLYWHEEL_GROUP + 90}px`}>
    <FlexBox justifyContent="center" margin="0 0 26px 0">
      <svg
        width="920"
        height="190"
        viewBox="0 0 920 190"
        role="img"
        aria-label="Order-book depth growing over time"
      >
        <path
          d="M0 181 C 293 176, 502 152, 655 97 C 781 52, 851 24, 920 6 L 920 190 L 0 190 Z"
          fill={colors.accent}
          opacity="0.18"
        />
        <path
          d="M0 181 C 293 176, 502 152, 655 97 C 781 52, 851 24, 920 6"
          fill="none"
          stroke={colors.accent}
          strokeWidth="6"
        />
      </svg>
    </FlexBox>
    <FlexBox justifyContent="space-between" alignItems="flex-start">
      <FlywheelEnd label="Upstream" partners={UPSTREAM} />
      <FlywheelEnd label="Downstream" partners={DOWNSTREAM} />
    </FlexBox>
  </Box>
);

/**
 * The commercial-viability rollout: three beats in time order along a rule.
 *
 * A **rollout, not a list** — the distinction is the page. Three bullets read
 * as three guesses about revenue; three beats on a timeline read as a plan
 * where each stage funds the next. The dot sits on the rule so the eye takes
 * the order before it reads any of the copy.
 */
type Beat = { when: string; headline: string; body: string };

const BEATS: Beat[] = [
  {
    when: "Now",
    headline: "DASMAC bootstraps the vaults.",
    body: "Liquidity operations are the company's business, and Dropset is the protocol they run on.",
  },
  {
    when: "Next",
    headline: "The exchange takes protocol fees.",
    body: "Illiquid pairs carry a natural premium, because nobody else is quoting them.",
  },
  {
    when: "Later",
    headline: "Derivatives, and hedging in particular.",
    body: "The ability to go short deepens the market making here, and serves treasury flows as payments come onchain.",
  },
];

const Rollout = () => (
  <Box margin="44px 0 0 0" width="1560px">
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
              height: "16px",
              width: "16px",
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
        <Box key={when} width="480px" margin="24px 0 0 0">
          <div
            style={{
              color: colors.accent,
              fontFamily: deckTheme.fonts.monospace,
              fontSize: "26px",
              lineHeight: 1.2,
            }}
          >
            {when}
          </div>
          <div
            style={{
              color: colors.foreground,
              fontSize: "34px",
              lineHeight: 1.25,
              marginTop: "12px",
            }}
          >
            {headline}
          </div>
          <div
            style={{
              color: colors.mutedFg,
              fontSize: "24px",
              lineHeight: 1.35,
              marginTop: "12px",
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
 * The open-venue contrast, on page 7. This is the page that used to be a row of
 * competitor logos, and the whole v2 rule is that **no competitor is named or
 * shown** — so the argument is made structurally instead: a gated panel beside
 * an open one, each captioned with a full sentence. Nothing here identifies who
 * the gated panel is, which is exactly the intent; the names live in the spec's
 * appendix, for conversation.
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
          // A wall: participants outside it, none of them connected through.
          <>
            <rect
              x="248"
              y="6"
              width="8"
              height="108"
              fill={tint}
              opacity="0.9"
            />
            {[40, 100, 160].map((x) => (
              <circle key={x} cx={x} cy="60" r="12" fill={tint} opacity="0.5" />
            ))}
            {[360, 420, 480].map((x) => (
              <circle key={x} cx={x} cy="60" r="12" fill={tint} opacity="0.5" />
            ))}
          </>
        ) : (
          // A hub: every participant connected to the same book. Node
          // positions are written out rather than derived — an alternating
          // top/bottom fan is easier to read as six literal points than as an
          // index parity trick, and it stays obvious which end is which.
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
 * This is the deck's one long-copy component, deliberately. Page 8 is the last
 * page and stays on screen after the talk ends, so full sentences someone can
 * read in depth are correct here and nowhere else — and the credentials are
 * stated as a flex rather than a modest line, which is what round-1 feedback
 * asked for.
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
  <Box margin="0 34px" width="640px">
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
      fontSize="24px"
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
          <Box margin="0 0 44px 0">
            <Wordmark width={860} />
          </Box>
          <Statement fontSize="88px">Where currency trades onchain.</Statement>
          {/* "Built by DASMAC" is on-slide as well as in the footer: the title
              is where the company/protocol distinction has to land first, and
              the footer credit is too small to carry it alone. */}
          <Text color="quaternary" fontSize="40px" margin="22px 0 0 0">
            Built by DASMAC.
          </Text>
          <BannerPair />
        </SlideBody>
        <Notes>
          Dropset is where currency trades onchain. [The two dashed frames are
          placeholders for the DASMAC company banner and its Dropset protocol
          counterpart — they are a lockup proposal, not final art, and come out
          before this is presented.]
        </Notes>
      </Slide>

      {/* 2 — The gap */}
      <Slide>
        <SlideBody>
          <Eyebrow>The gap</Eyebrow>
          <Statement fontSize="64px">
            Foreign exchange is the biggest market on earth, and less than 10% of
            the world’s currencies are available on Solana today.
          </Statement>
          <Screenshot
            src="/screens/currencies-listed.png"
            width={1000}
            alt="14 of 162 currencies represented on Solana; 148 not yet listed"
            source="dropset.io/currencies"
          />
        </SlideBody>
        <Notes>
          Foreign exchange is over nine trillion dollars a day. But it trades
          only 24/5, its liquidity is fragmented across obfuscated
          over-the-counter desks, and less than ten percent of the world’s
          currencies are even available on Solana today — fourteen out of a
          hundred and sixty-two, and that count is live on the site, where this
          is from. Every currency should be connectable to every other one, and
          that’s what we’re building. To be precise about it: we don’t issue
          currencies — issuers create them, and Dropset is where they trade.
        </Notes>
      </Slide>

      {/* 3 — Live today */}
      <Slide>
        <SlideBody>
          <Eyebrow>Live today</Eyebrow>
          <Statement>
            Dropset is settling real trades on mainnet today.
          </Statement>
          {/* The swap flow as three stills, in order — this is the beat that
              used to be the mainnet recording. Only the globe capture exists
              so far; the other two are placeholders sized to the shape the
              real captures need to be. */}
          <Sequence>
            <Box>
              <Box
                border={`1px solid ${colors.border}`}
                borderRadius="12px"
                padding="12px"
                backgroundColor={colors.muted}
              >
                <Image
                  src="/screens/mainnet-globe.png"
                  width={400}
                  alt="The Dropset globe, currencies pinned to the countries that issue them"
                />
              </Box>
              <StepCaption>Pick a currency on the globe</StepCaption>
            </Box>
            <Placeholder
              label="Mainnet screenshot: the route"
              width={424}
              height={356}
            />
            <Placeholder
              label="Mainnet screenshot: settled"
              width={424}
              height={356}
            />
          </Sequence>
        </SlideBody>
        <Notes>
          This already works. Dropset is live on mainnet today, clearing real
          trades: settlement is atomic, the ramps are near instant, and the
          venue never closes. Solana is the start, not the end — it’s the most
          moneyness-conducive environment onchain. [Today we clear by routing
          through aggregators; the eCLOB on the next page is how we bootstrap
          the markets that have no liquidity yet. Don’t claim “most liquid”.]
        </Notes>
      </Slide>

      {/* 4 — The eCLOB */}
      <Slide>
        <SlideBody>
          <Eyebrow>The eCLOB</Eyebrow>
          <Statement fontSize="60px">
            Running a real market onchain used to be prohibitively expensive, so
            we made quoting nearly free.
          </Statement>
          {/* Top-aligned, not centre-aligned: the two captures are very
              different heights, and centring them left the short one floating
              beside the middle of the tall one with both captions adrift. */}
          <FlexBox alignItems="flex-start" justifyContent="center">
            <Box margin="0 24px 0 0">
              <Screenshot
                src="/screens/maker-tui.png"
                width={430}
                alt="The maker control panel: seven FX markets and a live book"
                caption="Market maker TUI"
              />
            </Box>
            <Box margin="0 0 0 24px">
              <Screenshot
                src="/screens/compute-units.png"
                width={430}
                alt="Compute units per instruction: a reprice costs 47, a reshape 59"
                caption="Reprice: 47 CU · reshape: 59 CU"
              />
              {/* The bootstrap-from-empty beat, as a static strip in the space
                  the shorter capture leaves. These four frames are the one
                  asset the rework couldn't produce from what's committed —
                  they need a localnet run captured at four moments. */}
              <Box margin="26px 0 0 0">
                <FlexBox justifyContent="flex-start">
                  {[
                    "Empty book",
                    "Makers on",
                    "Depth fills in",
                    "A trade fills",
                  ].map((label) => (
                    <Box key={label} margin="0 8px 0 0">
                      <Placeholder label={label} width={100} height={78} />
                    </Box>
                  ))}
                </FlexBox>
                <StepCaption>A market coming alive, from empty</StepCaption>
              </Box>
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          Running a real market onchain used to be prohibitively expensive. Gas
          made continuous quoting impossible, so everything before this was a
          band-aid. We’ve built order books before, so we built one that fits:
          on the eCLOB, repricing the whole book costs forty-seven compute
          units, and reshaping the ladder fifty-nine — on a chain that gives you
          two hundred thousand per instruction. That’s what lets us bootstrap a
          brand-new market and onboard makers fast: the book starts empty, the
          makers come on, real depth fills in within seconds, and then a trade
          fills against it.
        </Notes>
      </Slide>

      {/* 5 — How we grow */}
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
          public rather than ours alone. The wedge is the long tail of
          currencies as they come onchain: spreads are wide there, and an issuer
          arriving with no depth of their own needs a day-one liquidity partner.
          It scales from today’s basket toward full G7 coverage. And it has two
          ends we’ve already talked to — upstream, the issuers who mint a
          currency and need it to trade; downstream, the payments companies who
          need to buy FX to settle. Connect the two and the depth compounds.
        </Notes>
      </Slide>

      {/* 6 — Commercial viability */}
      <Slide>
        <SlideBody>
          <Eyebrow>Commercial viability</Eyebrow>
          <Statement fontSize="68px">
            This is a business at every stage, and each stage funds the next.
          </Statement>
          <Rollout />
        </SlideBody>
        <Notes>
          This is a business at every stage. Today DASMAC — the company —
          bootstraps the vaults the way Hyperliquid did, so the liquidity
          operations are ours and Dropset is the protocol they run on. Next, the
          exchange takes fees, with a natural premium on the illiquid pairs
          nobody else quotes. After that, derivatives — hedging in particular,
          the ability to go short, which itself deepens the market making on the
          venue and serves business treasury flows as payments come onchain.
        </Notes>
      </Slide>

      {/* 7 — Why the open venue wins */}
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
              label="A gated venue: participants outside a wall, none connected through it"
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
          serving Singaporean day traders, not the business that needs to settle
          an invoice in another currency. That’s also why we started on Solana:
          it’s the most moneyness-conducive environment onchain. [Names stay off
          the slide and out of the talk — the appendix has them if a question
          goes there.]
        </Notes>
      </Slide>

      {/* 8 — Team & close. The last page, and it stays up. */}
      <Slide>
        <SlideBody>
          <Eyebrow>The team</Eyebrow>
          <Statement fontSize="60px">
            DASMAC has built onchain exchanges before, and runs its own
            operations.
          </Statement>
          <FlexBox margin="44px 0 0 0" justifyContent="center" alignItems="flex-start">
            <Portrait
              src="/remote/team-alex.png"
              name="Alex Kahn"
              role="Founder, DASMAC"
              prior="prev. Cofounder, Econia Labs"
              bio="Has built two onchain exchanges, authored the Econia order book on Aptos, which cleared around $500M in volume, and wrote the Solana Opcode Guide — the playbook for squeezing performance out of Solana programs, and the reason quoting on the eCLOB costs double-digit compute units."
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy Sosa"
              role="Operations, DASMAC"
              prior="prev. EA, Dragonfly Capital"
              bio="Owns operations end-to-end: banking relationships, the stablecoin providers, the onramps, and accounting. This is the work that gets an FX venue integrated with the rails money actually moves on, and it has a dedicated owner rather than being a founder’s side task."
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          This is DASMAC. I’ve built two onchain exchanges, including an order
          book — I authored Econia on Aptos, which cleared around five hundred
          million in volume, and wrote the Solana Opcode Guide, the playbook for
          squeezing performance out of Solana programs. Judy owns operations
          end-to-end: banking, the stablecoin providers, onramps, and
          accounting. Dropset — where currency trades onchain. [Leave this page
          up; it is written to be read after the talk ends.]
        </Notes>
      </Slide>
    </Deck>
  );
}
