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
import { DemoVideo } from "@/components/DemoVideo";
import { colors, deckTheme } from "@/theme/tokens";

/**
 * The demo-day pitch deck — a ~2-minute accelerator pitch built around two
 * recorded product demos. Slides are minimal backdrops the presenter talks
 * over; the full spoken script lives in each slide's `<Notes>` (presenter
 * mode, `⌘⇧P` — a bare `p` does nothing), never on the slide itself. Route
 * name is public-facing (`/demo-v1`); internal ticket ids never appear here
 * or in the URL.
 *
 * The copy follows `../../demo-v1-spec.md`, which is the source of truth for
 * it: one big sentence and one big visual per page, with the nuance kept off
 * the slides and in that doc's appendices. Edit the spec first, then the deck.
 *
 * Eight pages: the gap, then the mainnet demo (it already works), then the
 * eCLOB (we're just getting started), then the honest threat and its answer,
 * how we grow, and the team. The two demo beats are **recorded videos**, cued
 * by the badge on those pages, so nothing depends on a live network.
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
 * Persistent footer: wordmark on the left, the DASMAC credit in the middle
 * (mirroring the frontend's own footer), progress dots on the right. The
 * DASMAC mark is a transparent PNG, so unlike the Dropset one it needs no
 * blend to sit on the dark backdrop.
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
      <Wordmark width={150} />
    </Box>
    <FlexBox alignItems="center">
      <Text color="quaternary" fontSize="16px" margin="0 10px 0 0">
        Courtesy of
      </Text>
      <img src="/dasmac-wordmark.png" alt="DASMAC" width={78} />
    </FlexBox>
    <Box padding="0 1.25em">
      <Progress color={colors.accent} size={8} />
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
    padding="0 0 76px 0"
  >
    {children}
  </FlexBox>
);

// Small monospace kicker that labels each content slide.
const Eyebrow = ({ children }: { children: React.ReactNode }) => (
  <Text
    color="secondary"
    fontFamily="monospace"
    fontSize="22px"
    margin="0 0 8px 0"
  >
    {children}
  </Text>
);

/**
 * The one big sentence a page is built around — the only prose on a slide.
 * Sized down from the theme's `h1` so a full sentence still fits on one or two
 * lines, and width-capped so it wraps at a readable measure instead of
 * spanning the whole slide.
 */
const Statement = ({
  children,
  fontSize = "56px",
}: {
  children: React.ReactNode;
  fontSize?: string;
}) => (
  <Heading fontSize={fontSize} margin="0" maxWidth="1100px">
    {children}
  </Heading>
);

/**
 * The recordings behind the two demo beats. Both point at the same placeholder
 * Short for now; when the real captures exist, each beat gets its own id (and
 * `portrait: false` if it's a landscape screen recording).
 *
 * Naming the network on the badge is deliberate — the mainnet demo is the real
 * venue, the localnet one is a market bootstrapped from empty, and conflating
 * the two would overstate what is live.
 */
const DEMOS = {
  mainnet: { videoId: "blHXBmt6RI0", portrait: true },
  localnet: { videoId: "blHXBmt6RI0", portrait: true },
} as const;

// The badge is small on purpose: it labels the page, it isn't the page. Clicking
// it plays the recording over the whole window (see `DemoVideo`). `margin` is a
// prop because the badge sits under its visual on one page and beside it on
// another, where a stacked top offset would be from the wrong layout.
const DemoBadge = ({
  network,
  margin = "26px 0 0 0",
}: {
  network: keyof typeof DEMOS;
  margin?: string;
}) => (
  <FlexBox margin={margin}>
    <DemoVideo network={network} {...DEMOS[network]} />
  </FlexBox>
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
  <Box margin="30px 0 0 0">
    <Box
      border={`1px solid ${colors.border}`}
      borderRadius="10px"
      padding="14px 18px"
      backgroundColor={colors.muted}
    >
      <Image src={src} width={width} alt={alt} />
    </Box>
    {source ? (
      <Link href={`https://${source}`} fontSize="20px" margin="8px 0 0 0">
        {source}
      </Link>
    ) : null}
    {caption ? (
      <Text color="quaternary" fontSize="20px" margin="8px 0 0 0">
        {caption}
      </Text>
    ) : null}
  </Box>
);

/**
 * A row of captioned logos — the one visual on the three pages that are about
 * other companies (the threats, the incumbents, the demand).
 *
 * The marks are each company's own, so they come in two shapes: square icons
 * and logotypes four times as wide as they are tall. Tiles are therefore
 * **height-matched with the width left free** — forcing every mark into one
 * square is what squashed the wide ones flat. Every source here is either
 * transparent or already dark-backed, so tiles carry no fill of their own and
 * the marks sit straight on the deck's black.
 *
 * Captions carry the name the presenter says, which isn't always what the mark
 * says — Arc is Circle's, so Circle's logo is what an audience recognizes.
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
const LOGO_CAPTION_ROOM = 56;
const LOGO_GUTTER = 10;

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
        width={`${width + LOGO_CAPTION_ROOM}px`}
      >
        {/* Every tile in a row is the same box; the mark is contained inside
            it rather than sized to it, so a wide logotype and a square icon
            share a footprint without either being stretched.
            Tile, caption and note all sit flush to the column's left edge —
            centring the tile inside a column wider than itself, while the text
            below started at the column edge, left the two visibly out of
            line. */}
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
        {/* Captions are plain elements, not `Text`: Spectacle's carries a
            theme margin of its own that opens a gap between a name and its
            note far larger than either margin asks for, and on the flywheel
            that pushed the last line into the footer. */}
        <div
          style={{
            color: tint ?? colors.mutedFg,
            fontFamily: deckTheme.fonts.monospace,
            fontSize: "20px",
            lineHeight: 1.2,
            marginTop: "12px",
          }}
        >
          {name}
        </div>
        {note ? (
          <div
            style={{
              color: colors.mutedFg,
              fontSize: "17px",
              lineHeight: 1.2,
              marginTop: "5px",
            }}
          >
            {note}
          </div>
        ) : null}
      </Box>
    ))}
  </FlexBox>
);

// Page 5 — the chains that could decide FX is theirs, alphabetically, so the
// row implies no ranking among them.
const THREATS: Logo[] = [
  { name: "Arc", src: "/remote/logo-circle.svg" },
  { name: "Canton", src: "/remote/logo-canton.svg" },
  { name: "Tempo", src: "/remote/logo-tempo.svg" },
];

// Page 6 — the big Solana venues, whose attention is elsewhere. Sourced from
// each project's own asset (Jupiter, Meteora) or the Solana token list, so
// every mark is transparent and reads on black rather than in a white box.
const INCUMBENTS: Logo[] = [
  { name: "Jupiter", src: "/remote/logo-jupiter.svg" },
  { name: "Orca", src: "/remote/logo-orca.png" },
  { name: "Raydium", src: "/remote/logo-raydium.png" },
  { name: "Meteora", src: "/remote/logo-meteora.svg" },
  { name: "pump.fun", src: "/remote/logo-pump-fun.svg" },
];

/**
 * Page 7 — the two ends of the flywheel. Issuers sit **upstream**: they mint
 * the currency and need it to trade. Payments companies sit **downstream**:
 * they consume the liquidity to settle real invoices. Naming both directions
 * is the point of the page — a venue needs each end to bootstrap.
 */
const UPSTREAM: Logo[] = [
  { name: "Loon", src: "/remote/logo-cadc.png", note: "CADC issuer" },
  { name: "AUDD Digital", src: "/remote/logo-audd.png", note: "AUDD issuer" },
];

const DOWNSTREAM: Logo[] = [
  { name: "Altitude", src: "/remote/logo-altitude.png", note: "Banking" },
  { name: "CargoBill", src: "/remote/logo-cargobill.png", note: "Supply chain" },
];

/**
 * The flywheel, as its two ends (page 7): the issuers whose currency needs a
 * market, and the payments companies that need to buy FX. The curve behind
 * them is depth growing once both ends are connected — an inline SVG, so it
 * stays crisp at projector size with no asset pipeline.
 */
// Square: every mark on this page is a pure icon, so `TILE` matches the tile's
// own height (`LOGO_TILE_PADDING` + the 58px image cap passed below).
const FLYWHEEL_TILE = 104;
// A group is two tile columns, so its heading rule spans exactly its own pair.
// Derived from `LogoRow`'s constants rather than repeating their values.
const FLYWHEEL_GROUP =
  2 * (FLYWHEEL_TILE + LOGO_CAPTION_ROOM + 2 * LOGO_GUTTER);
const FLYWHEEL_WIDTH = 2 * FLYWHEEL_GROUP + 60;

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
        fontSize: "21px",
        lineHeight: 1.2,
        marginBottom: "10px",
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
      margin="18px 0 0 0"
    />
  </Box>
);

const Flywheel = () => (
  <Box margin="18px 0 0 0" width={`${FLYWHEEL_WIDTH}px`}>
    {/* The curve is centred over both ends of the flywheel — it's the depth
        that appears once the two are connected. Wide enough to carry the page
        on its own: when it was narrow and a divider ran down from under it,
        the pair read as a chart mounted on a stick. */}
    <FlexBox justifyContent="center" margin="0 0 22px 0">
      <svg
        width="660"
        height="150"
        viewBox="0 0 660 150"
        role="img"
        aria-label="Order-book depth growing over time"
      >
        <path
          d="M0 143 C 210 139, 360 120, 470 77 C 560 41, 610 19, 660 5 L 660 150 L 0 150 Z"
          fill={colors.accent}
          opacity="0.18"
        />
        <path
          d="M0 143 C 210 139, 360 120, 470 77 C 560 41, 610 19, 660 5"
          fill="none"
          stroke={colors.accent}
          strokeWidth="5"
        />
      </svg>
    </FlexBox>
    {/* No divider between the ends: each heading's rule already brackets its
        own pair, and the vertical line only survived as the stick the curve
        appeared to stand on. */}
    <FlexBox justifyContent="space-between" alignItems="flex-start">
      <FlywheelEnd label="Upstream" logos={UPSTREAM} />
      <FlywheelEnd label="Downstream" logos={DOWNSTREAM} />
    </FlexBox>
  </Box>
);

/**
 * Team headshots, mirrored from the marketing site at build time, captioned.
 * Left square and unframed — the sources are already square, and both photos
 * are shot on a dark background that reads as part of the slide.
 */
const Portrait = ({
  src,
  name,
  role,
  focus,
  prior,
}: {
  src: string;
  name: string;
  role: string;
  focus: string;
  prior: string;
}) => (
  <Box margin="0 32px">
    <Image src={src} width={140} height={140} alt={name} />
    <Text fontSize="26px" margin="14px 0 0 0">
      {name}
    </Text>
    <Text fontSize="21px" margin="4px 0 0 0">
      {role}
    </Text>
    <Text color="quaternary" fontSize="19px" margin="2px 0 0 0">
      {focus}
    </Text>
    <Text
      color="secondary"
      fontFamily="monospace"
      fontSize="18px"
      margin="6px 0 0 0"
    >
      {prior}
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
            <Wordmark width={700} />
          </Box>
          <Statement fontSize="72px">Forex on Solana.</Statement>
        </SlideBody>
        <Notes>
          Dropset is onchain Forex on Solana — providing open and efficient
          exchange of the world’s currencies at scale.
        </Notes>
      </Slide>

      {/* 2 — The gap */}
      <Slide>
        <SlideBody>
          <Eyebrow>The gap</Eyebrow>
          <Statement>
            The biggest market on earth barely exists onchain.
          </Statement>
          <Screenshot
            src="/screens/currencies-listed.png"
            width={720}
            alt="14 of 162 currencies represented on Solana; 148 not yet listed"
            source="dropset.io/currencies"
          />
        </SlideBody>
        <Notes>
          Foreign exchange is over nine trillion dollars a day, and it trades
          24/5 — but onchain it has no liquid home. Only about fourteen of the
          world’s currencies are represented on Solana today, with the euro
          driving most of the volume — that count is live on
          dropset.io/currencies, where this is from. Settle FX through Solana
          and you get atomic settlement and near-instant on- and off-ramps.
        </Notes>
      </Slide>

      {/* 3 — The mainnet demo */}
      <Slide>
        <SlideBody>
          <Eyebrow>Live on mainnet</Eyebrow>
          <Statement>But Dropset is changing this.</Statement>
          {/* Badge beside the capture rather than under it: this page's visual
              is tall, so a badge below it lands on the footer. */}
          <FlexBox alignItems="center" justifyContent="center">
            <Screenshot
              src="/screens/mainnet-globe.png"
              width={400}
              alt="The Dropset globe, currencies pinned to the countries that issue them"
            />
            <Box margin="0 0 0 44px">
              <DemoBadge network="mainnet" margin="0" />
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          But Dropset is changing that, and it already works: it’s live on
          mainnet today, clearing real trades by routing FX through aggregators
          — pick the currency you want on the globe and the swap settles. [Play
          the mainnet demo video.]
        </Notes>
      </Slide>

      {/* 4 — The eCLOB */}
      <Slide>
        <SlideBody>
          <Eyebrow>The eCLOB</Eyebrow>
          <Statement fontSize="44px">
            Institutional-grade atomic settlement: order book transparency,
            propAMM efficiency.
          </Statement>
          {/* Top-aligned, not centre-aligned: the two captures are very
              different heights, and centring them left the short one floating
              beside the middle of the tall one with both captions adrift. The
              badge sits at the foot of the right column, which is the free
              space that column's shorter capture leaves. */}
          <FlexBox alignItems="flex-start" justifyContent="center">
            <Box margin="0 20px 0 0">
              <Screenshot
                src="/screens/maker-tui.png"
                width={330}
                alt="The maker control panel: seven FX markets and a live EURC book"
                caption="Market maker TUI"
              />
            </Box>
            <Box margin="0 0 0 20px">
              <Screenshot
                src="/screens/compute-units.png"
                width={330}
                alt="Compute units per instruction: a reprice costs 47, a reshape 491"
                caption="Reprice: 47 CU · reshape: 491 CU"
              />
              <DemoBadge network="localnet" />
            </Box>
          </FlexBox>
        </SlideBody>
        <Notes>
          The routing works today, but the markets that don’t exist yet need a
          venue — so we built one. The eCLOB gives you the liquidity guarantees
          of a central limit order book with quote updates as cheap as a
          propAMM: a maker repricing the whole book costs forty-seven compute
          units, reshaping the ladder about five hundred. That’s what lets us
          bootstrap a brand-new market and onboard makers fast. [Play the
          localnet demo video: the book starts empty, the maker bots come on,
          and real depth fills in within seconds — then a trade fills against
          it.]
        </Notes>
      </Slide>

      {/* 5 — Why this will fail */}
      <Slide>
        <SlideBody>
          <Eyebrow>Permissioned distribution</Eyebrow>
          {/* The asterisk is the joke: it promises a footnote the audience
              doesn't get until the next page's title answers it. */}
          <Statement>Why Dropset will fail*</Statement>
          <LogoRow logos={THREATS} width={210} height={46} tint={colors.sell} />
        </SlideBody>
        <Notes>
          The honest risk: everyone wants onchain settlement, and the ones with
          distribution are permissioning it. Arc and Tempo are building
          payment-and-settlement rails, and Canton is doing regulated onchain
          markets. Any of them could decide FX is theirs, and each arrives with
          the customers already on it.
        </Notes>
      </Slide>

      {/* 6 — Why it will work */}
      <Slide>
        <SlideBody>
          <Eyebrow>*Why Dropset won’t actually fail</Eyebrow>
          <Statement fontSize="48px">
            But Dropset liquidity is public, and the biggest Solana DEXes face
            an innovator’s dilemma (SOL, memes).
          </Statement>
          {/* Square tiles: these are all pure icons. The threats row above
              keeps wide ones, since those marks are logotypes. */}
          <LogoRow logos={INCUMBENTS} width={110} height={64} />
        </SlideBody>
        <Notes>
          Their liquidity isn’t public: it sits inside private or permissioned
          rails, where you can’t make a market unless they let you. Dropset’s
          is. And the venues that are public — Jupiter, Orca, Raydium, Meteora,
          pump.fun — are chasing SOL and memes, because that’s where the volume
          is today. It’s a classic innovator’s dilemma: FX is too small to move
          them and big enough for us. Dropset is the open, neutral, composable
          venue — anyone can quote, anyone can trade, any app can integrate —
          and we’re beating everyone to it.
        </Notes>
      </Slide>

      {/* 7 — How we grow */}
      <Slide>
        <SlideBody>
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="52px">
            Vaults bootstrap a public FX liquidity flywheel.
          </Statement>
          <Flywheel />
        </SlideBody>
        <Notes>
          We seed the markets ourselves the way Hyperliquid did — our vaults
          bootstrap each book, and anyone can top them off with inventory, so
          the flywheel is public rather than ours alone. It has two ends, and
          we’re talking to partners at both. Upstream, issuers like Loon, who
          issues CADC, and AUDD Digital: they mint a currency and need it to
          actually trade. Downstream, the demand — Colosseum partners like
          Altitude in banking and CargoBill in supply chain, who need to buy FX
          onchain to settle. Connect the two ends and the depth compounds.
        </Notes>
      </Slide>

      {/* 8 — Team & close */}
      <Slide>
        <SlideBody>
          <Eyebrow>Team</Eyebrow>
          <Statement>Built by people who’ve built exchanges.</Statement>
          <FlexBox margin="40px 0 0 0" justifyContent="center">
            <Portrait
              src="/remote/team-alex.png"
              name="Alex Kahn"
              role="Founder, DASMAC"
              focus="Product · exchange design"
              prior="prev. Cofounder, Econia Labs"
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy Sosa"
              role="Operations, DASMAC"
              focus="Stablecoin rails · onramps · accounting"
              prior="prev. Dragonfly Capital Partners"
            />
          </FlexBox>
        </SlideBody>
        <Notes>
          I’ve built two onchain exchanges already, including an order book — I
          authored Econia on Aptos, which cleared around five hundred million in
          volume, and wrote the Solana Opcode Guide, the playbook for squeezing
          performance out of Solana programs. Judy came with me from Econia
          Labs, and was at Dragonfly before that; she owns operations end-to-end
          — banking, the stablecoin providers, onramps, and accounting. Dropset
          — Forex on Solana.
        </Notes>
      </Slide>
    </Deck>
  );
}
