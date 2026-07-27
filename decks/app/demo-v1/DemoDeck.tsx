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
 * The demo-day pitch deck — a ~2-minute accelerator pitch built around two
 * recorded product demos. Slides are minimal backdrops the presenter talks
 * over; the full spoken script lives in each slide's `<Notes>` (presenter
 * mode, `p`), never on the slide itself. Route name is public-facing
 * (`/demo-v1`); internal ticket ids never appear here or in the URL.
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

// Persistent footer: wordmark on the left, progress dots on the right.
const template = () => (
  <FlexBox
    justifyContent="space-between"
    position="absolute"
    bottom={0}
    width={1}
    zIndex={1}
  >
    <Box padding="0 1.25em">
      <Image src="/dropset-wordmark.png" width={110} />
    </Box>
    <Box padding="0 1.25em">
      <Progress color={colors.accent} size={8} />
    </Box>
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
 * Marks a page whose visual is a recorded demo the presenter plays, and says
 * which network it was recorded on — the mainnet demo is the real venue, the
 * localnet one is a market bootstrapped from empty, and conflating the two
 * would overstate what is live. Deliberately small: a label, not the page.
 */
const DemoBadge = ({ network }: { network: string }) => (
  <FlexBox margin="26px 0 0 0">
    <Box
      border={`1px solid ${colors.accent}`}
      borderRadius="6px"
      padding="7px 14px"
    >
      <Text
        color="secondary"
        fontFamily="monospace"
        fontSize="19px"
        margin="0"
      >
        ▶ demo video · {network}
      </Text>
    </Box>
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
 * The marks come from each company's own site or icon, so they arrive with
 * wildly different backgrounds: white JPEGs, black squares, transparent SVGs.
 * Equal-size rounded tiles with `overflow: hidden` give the row its rhythm
 * regardless, the way a row of app icons reads. Captions carry the name the
 * presenter says, which isn't always what the mark says — Arc is Circle's, so
 * Circle's logo is what an audience actually recognizes.
 */
type Logo = { name: string; src: string; note?: string };

const LogoRow = ({
  logos,
  size = 88,
  tint,
  margin = "34px 0 0 0",
}: {
  logos: Logo[];
  size?: number;
  tint?: string;
  margin?: string;
}) => (
  <FlexBox margin={margin} justifyContent="center" alignItems="flex-start">
    {logos.map(({ name, src, note }) => (
      <Box key={name} margin="0 20px" width={`${size + 40}px`}>
        <FlexBox justifyContent="center">
          <Box
            width={`${size}px`}
            height={`${size}px`}
            borderRadius="16px"
            overflow="hidden"
            border={`1px solid ${tint ?? colors.border}`}
          >
            <Image src={src} width={size} height={size} alt={name} />
          </Box>
        </FlexBox>
        <Text
          color={tint ?? colors.mutedFg}
          fontFamily="monospace"
          fontSize="20px"
          margin="12px 0 0 0"
        >
          {name}
        </Text>
        {note ? (
          <Text color="quaternary" fontSize="17px" margin="2px 0 0 0">
            {note}
          </Text>
        ) : null}
      </Box>
    ))}
  </FlexBox>
);

// Page 5 — the chains that could decide FX is theirs.
const THREATS: Logo[] = [
  { name: "Arc", src: "/remote/logo-circle.svg" },
  { name: "Tempo", src: "/remote/logo-tempo.svg" },
  { name: "Canton", src: "/remote/logo-canton.svg" },
];

// Page 6 — the big Solana venues, whose attention is elsewhere.
const INCUMBENTS: Logo[] = [
  { name: "Jupiter", src: "/remote/logo-jupiter.jpg" },
  { name: "Orca", src: "/remote/logo-orca.png" },
  { name: "Raydium", src: "/remote/logo-raydium.jpg" },
];

/**
 * Page 7 — the two ends of the flywheel. Issuers sit **upstream**: they mint
 * the currency and need it to trade. Payments companies sit **downstream**:
 * they consume the liquidity to settle real invoices. Naming both directions
 * is the point of the page — a venue needs each end to bootstrap.
 */
const UPSTREAM: Logo[] = [
  { name: "CADC", src: "/remote/logo-cadc.png", note: "issuer" },
  { name: "AUDD", src: "/remote/logo-audd.png", note: "issuer" },
];

const DOWNSTREAM: Logo[] = [
  { name: "Altitude", src: "/remote/logo-altitude.png", note: "payments" },
  { name: "CargoBill", src: "/remote/logo-cargobill.jpg", note: "payments" },
];

/**
 * The flywheel, as its two ends (page 7): the issuers whose currency needs a
 * market, and the payments companies that need to buy FX. The curve behind
 * them is depth growing once both ends are connected — an inline SVG, so it
 * stays crisp at projector size with no asset pipeline.
 */
const Flywheel = () => (
  <Box margin="18px 0 0 0">
    <svg
      width="620"
      height="120"
      viewBox="0 0 620 120"
      role="img"
      aria-label="Order-book depth growing over time"
    >
      <path
        d="M0 112 C 180 108, 330 88, 430 56 C 510 30, 570 14, 620 6 L 620 120 L 0 120 Z"
        fill={colors.accent}
        opacity="0.14"
      />
      <path
        d="M0 112 C 180 108, 330 88, 430 56 C 510 30, 570 14, 620 6"
        fill="none"
        stroke={colors.accent}
        strokeWidth="4"
      />
    </svg>
    <FlexBox justifyContent="center" alignItems="flex-start">
      <Box margin="0 26px 0 0">
        <Text
          color="secondary"
          fontFamily="monospace"
          fontSize="19px"
          margin="0"
        >
          upstream
        </Text>
        <LogoRow logos={UPSTREAM} size={72} margin="14px 0 0 0" />
      </Box>
      <Box width="1px" height="150px" backgroundColor={colors.border} />
      <Box margin="0 0 0 26px">
        <Text
          color="secondary"
          fontFamily="monospace"
          fontSize="19px"
          margin="0"
        >
          downstream
        </Text>
        <LogoRow logos={DOWNSTREAM} size={72} margin="14px 0 0 0" />
      </Box>
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
  prior,
}: {
  src: string;
  name: string;
  role: string;
  prior: string;
}) => (
  <Box margin="0 32px">
    <Image src={src} width={140} height={140} alt={name} />
    <Text fontSize="26px" margin="14px 0 0 0">
      {name}
    </Text>
    <Text color="quaternary" fontSize="20px" margin="0">
      {role}
    </Text>
    <Text
      color="secondary"
      fontFamily="monospace"
      fontSize="18px"
      margin="4px 0 0 0"
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
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Box margin="0 0 40px 0">
            <Image src="/dropset-wordmark.png" width={520} />
          </Box>
          <Statement fontSize="72px">Forex on Solana.</Statement>
        </FlexBox>
        <Notes>
          Dropset is onchain Forex on Solana — providing open and efficient
          exchange of the world’s currencies at scale.
        </Notes>
      </Slide>

      {/* 2 — The gap */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
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
        </FlexBox>
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
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
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
              <DemoBadge network="mainnet" />
            </Box>
          </FlexBox>
        </FlexBox>
        <Notes>
          But Dropset is changing that, and it already works: we’re live on
          mainnet today, clearing real trades by routing FX through aggregators
          — pick the currency you want on the globe and the swap settles. [Play
          the mainnet demo video.]
        </Notes>
      </Slide>

      {/* 4 — The eCLOB */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>The eCLOB</Eyebrow>
          <Statement fontSize="44px">
            And we’re just getting started: order-book depth, propAMM-cheap
            quotes.
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
        </FlexBox>
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
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Why this will fail</Eyebrow>
          <Statement>Why this will fail: permissioned distribution.</Statement>
          <LogoRow logos={THREATS} tint={colors.sell} />
        </FlexBox>
        <Notes>
          The honest risk: everyone wants onchain settlement, and the ones with
          distribution are the ones who get to permission it. Arc and Tempo are
          building payment-and-settlement rails, and Canton is doing regulated
          onchain markets. Any of them could decide FX is theirs, and each
          arrives with the customers already on it.
        </Notes>
      </Slide>

      {/* 6 — Why it will work */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Why it will work</Eyebrow>
          <Statement fontSize="50px">
            But their liquidity isn’t public — and the big Solana DEXes focus on
            SOL, memes.
          </Statement>
          <LogoRow logos={INCUMBENTS} />
        </FlexBox>
        <Notes>
          But their liquidity isn’t public: it sits inside private or
          permissioned rails, where you can’t make a market unless they let you.
          And the venues that are public — Jupiter, Orca, Raydium — are focused
          on SOL and memes, because that’s where the volume is today. It’s a
          classic innovator’s dilemma: FX is too small to move them and big
          enough for us. Dropset is the open, neutral, composable venue — anyone
          can quote, anyone can trade, any app can integrate — and we’re beating
          everyone to it.
        </Notes>
      </Slide>

      {/* 7 — How we grow */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="52px">
            We bootstrap a public liquidity flywheel.
          </Statement>
          <Flywheel />
        </FlexBox>
        <Notes>
          We seed the markets ourselves the way Hyperliquid did — we bootstrap
          the vaults, and anyone can top them off with inventory, so the
          flywheel is public rather than ours alone. It has two ends, and
          we’ve talked with both. Upstream are the issuers — CADC, AUDD — who
          mint a currency and need it to actually trade. Downstream are the
          payments companies — Colosseum partners like Altitude and CargoBill —
          who need to buy FX onchain to settle. Connect the two ends and the
          depth compounds.
        </Notes>
      </Slide>

      {/* 8 — Team & close */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Team</Eyebrow>
          <Statement>Built by people who’ve built exchanges.</Statement>
          <FlexBox margin="40px 0 0 0" justifyContent="center">
            <Portrait
              src="/remote/team-alex.png"
              name="Alex"
              role="Product · exchange design"
              prior="Cofounder, Econia Labs"
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy"
              role="Operations · stablecoin rails"
              prior="prev. Dragonfly Capital Partners"
            />
          </FlexBox>
        </FlexBox>
        <Notes>
          I’ve built two onchain exchanges already, including an order book — I
          authored Econia on Aptos, which cleared around five hundred million in
          volume, and wrote the Solana Opcode Guide, the playbook for squeezing
          performance out of Solana programs. Judy owns operations end-to-end —
          banking, the stablecoin providers, onramps, and accounting. Dropset —
          Forex on Solana.
        </Notes>
      </Slide>
    </Deck>
  );
}
