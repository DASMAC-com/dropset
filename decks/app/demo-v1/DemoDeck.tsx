"use client";

import {
  Box,
  Deck,
  FlexBox,
  Heading,
  Image,
  Notes,
  Progress,
  Slide,
  Text,
} from "spectacle";
import { colors, deckTheme } from "@/theme/tokens";

/**
 * The demo-day pitch deck — a ~2-minute accelerator pitch built around a
 * live product demo. Slides are minimal backdrops the presenter talks over;
 * the full spoken script lives in each slide's `<Notes>` (presenter mode,
 * `p`), never on the slide itself. Route name is public-facing (`/demo-v1`);
 * internal ticket ids never appear here or in the URL.
 *
 * The copy is reconciled to `../../demo-v1-spec.md`, which is the source of
 * truth: ten pages, one big sentence and one big visual per page, nuance kept
 * off the slides and in that doc's appendices. Edit the spec first, then the
 * deck. Timing guide: pages 1–3 ≈ 32s, the two demo beats (4–5) ≈ 50s,
 * pages 6–10 ≈ 38s.
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

// A live-demo cue the presenter reads as "run the demo here". The command is
// on the slide deliberately: it's the same one both demo beats run.
const DemoCue = ({ children }: { children: React.ReactNode }) => (
  <Box
    border={`1px solid ${colors.accent}`}
    borderRadius="8px"
    padding="12px 20px"
    margin="36px 0 0 0"
  >
    <Text
      color="secondary"
      fontFamily="monospace"
      fontSize="24px"
      margin="0"
    >
      ▶ Live demo · {children}
    </Text>
  </Box>
);

// Frames a screen capture so it reads as a window rather than floating art.
const Screenshot = ({
  src,
  width,
  alt,
}: {
  src: string;
  width: number;
  alt: string;
}) => (
  <Box
    border={`1px solid ${colors.border}`}
    borderRadius="10px"
    padding="18px 24px"
    margin="36px 0 0 0"
    backgroundColor={colors.muted}
  >
    <Image src={src} width={width} alt={alt} />
  </Box>
);

/**
 * A book with depth on both sides (page 3) — asks above, bids below, size
 * growing away from top of book. Drawn rather than captured, and deliberately
 * unlabelled: it's the *shape* of order-book depth, not a claim about a
 * specific market's prices or size.
 */
const OrderBookLadder = () => (
  <Box margin="34px 0 0 0">
    {[78, 62, 48, 34, 22].map((size, i) => (
      <FlexBox key={`ask-${i}`} justifyContent="center" margin="0 0 7px 0">
        <Box width="640px">
          <Box width={`${size}%`} height="16px" backgroundColor={colors.sell} />
        </Box>
      </FlexBox>
    ))}
    <Box width="640px" height="1px" backgroundColor={colors.border} />
    {[22, 34, 48, 62, 78].map((size, i) => (
      <FlexBox key={`bid-${i}`} justifyContent="center" margin="7px 0 0 0">
        <Box width="640px">
          <Box width={`${size}%`} height="16px" backgroundColor={colors.buy} />
        </Box>
      </FlexBox>
    ))}
  </Box>
);

/**
 * The competitors, as a wall closing in (page 7). Rendered as type rather than
 * logos: the deck ships no third-party marks, and the names are what the
 * presenter says out loud anyway.
 */
const CompetitorWall = () => (
  <FlexBox margin="40px 0 0 0" justifyContent="center">
    {["Arc", "Tempo", "Canton"].map((name) => (
      <Box
        key={name}
        border={`1px solid ${colors.sell}`}
        borderRadius="10px"
        padding="18px 34px"
        margin="0 12px"
      >
        <Text
          color={colors.sell}
          fontFamily="monospace"
          fontSize="34px"
          margin="0"
        >
          {name}
        </Text>
      </Box>
    ))}
  </FlexBox>
);

/**
 * One open venue against a row of closed ones (page 8) — the visual answer to
 * the wall on the previous page. Doors are drawn, not photographed, so the
 * shape reads instantly at the back of a room.
 */
const OpenVsClosed = () => (
  <FlexBox margin="44px 0 0 0" alignItems="flex-end" justifyContent="center">
    {[0, 1, 2, 3].map((i) => (
      <Box
        key={i}
        width="88px"
        height="150px"
        border={`1px solid ${colors.border}`}
        borderRadius="6px 6px 0 0"
        backgroundColor={colors.muted}
        margin="0 10px"
      />
    ))}
    <Box margin="0 10px">
      <Box
        width="118px"
        height="196px"
        border={`2px solid ${colors.accent}`}
        borderRadius="6px 6px 0 0"
        backgroundColor={colors.background}
      />
      <Text
        color="secondary"
        fontFamily="monospace"
        fontSize="22px"
        margin="10px 0 0 0"
      >
        Dropset
      </Text>
    </Box>
  </FlexBox>
);

/**
 * Depth growing from a seeded book (page 9), with the first sources of FX
 * demand named underneath. An inline SVG keeps it crisp at projector size and
 * needs no asset pipeline.
 */
const GrowthCurve = () => (
  <Box margin="36px 0 0 0">
    <svg
      width="720"
      height="230"
      viewBox="0 0 720 230"
      role="img"
      aria-label="Order-book depth growing over time"
    >
      <path
        d="M0 220 C 210 214, 380 176, 500 112 C 590 64, 660 28, 720 12 L 720 230 L 0 230 Z"
        fill={colors.accent}
        opacity="0.14"
      />
      <path
        d="M0 220 C 210 214, 380 176, 500 112 C 590 64, 660 28, 720 12"
        fill="none"
        stroke={colors.accent}
        strokeWidth="4"
      />
    </svg>
    <Text color="quaternary" fontSize="26px" margin="6px 0 0 0">
      Our vault seeds it · Altitude and Cargobill bring the flow
    </Text>
  </Box>
);

// Team headshots, mirrored from the marketing site at build time, captioned.
const Portrait = ({
  src,
  name,
  role,
  size,
}: {
  src: string;
  name: string;
  role: string;
  size: number;
}) => (
  <Box margin="0 32px">
    <Box
      width={`${size}px`}
      height={`${size}px`}
      borderRadius="50%"
      overflow="hidden"
      border={`1px solid ${colors.border}`}
    >
      <Image src={src} width={size} height={size} alt={name} />
    </Box>
    <Text fontSize="26px" margin="14px 0 0 0">
      {name}
    </Text>
    <Text color="quaternary" fontSize="20px" margin="0">
      {role}
    </Text>
  </Box>
);

export default function DemoDeck() {
  return (
    <Deck theme={deckTheme} template={template}>
      {/* 1 — Title */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Box margin="0 0 32px 0">
            <Image src="/dropset-wordmark.png" width={280} />
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
            width={780}
            alt="14 of 162 currencies represented on Solana; 148 not yet listed"
          />
        </FlexBox>
        <Notes>
          Foreign exchange is over nine trillion dollars a day, and it trades
          24/5 — but onchain it has no liquid home. Only about fourteen of the
          world’s currencies are represented on Solana today, with the euro
          driving most of the volume. Settle FX through Solana and you get
          atomic settlement and near-instant on- and off-ramps.
        </Notes>
      </Slide>

      {/* 3 — The eCLOB */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>The eCLOB</Eyebrow>
          <Statement>
            So we built the eCLOB: order-book depth, propAMM-cheap quotes.
          </Statement>
          <OrderBookLadder />
        </FlexBox>
        <Notes>
          Our edge is a new exchange design — the eCLOB. You get the liquidity
          guarantees of a central limit order book, but quote updates as cheap
          as a propAMM. That lets us bootstrap brand-new markets and onboard
          market makers far faster.
        </Notes>
      </Slide>

      {/* 4 — The stack: maker panel + a swap [demo · localnet] */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>The stack</Eyebrow>
          <Statement>
            Here’s the market-maker’s control panel — and a swap clearing.
          </Statement>
          <DemoCue>make demo</DemoCue>
        </FlexBox>
        <Notes>
          Let me show you the stack. This is our market-maker control panel —
          the TUI a maker uses to quote a book. And here’s the user side: I
          run a swap on the frontend and it clears. [Run `make demo`. If
          anything fails live, fall back to the recorded video.]
        </Notes>
      </Slide>

      {/* 5 — A market comes alive [demo · localnet] */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>A market comes alive</Eyebrow>
          <Statement>Empty book, makers on — real depth in seconds.</Statement>
          <DemoCue>make demo</DemoCue>
        </FlexBox>
        <Notes>
          Now watch a brand-new market come alive. The book starts empty — I
          turn the maker bots on, and top-of-book fills in live. Then I trade
          against real eCLOB depth and it fills the size. This is the
          market-maker’s view; the frontend is the user’s view.
          [Optional flourish, if time allows: from the TUI, reshape the ladder
          or reprice the whole book in a single instruction.]
        </Notes>
      </Slide>

      {/* 6 — Traction */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Traction</Eyebrow>
          <Statement fontSize="62px">
            And just like that, my laptop is quoting FX on Solana.
          </Statement>
        </FlexBox>
        <Notes>
          And just like that, my laptop is quoting FX on Solana. This isn’t
          only a demo — Dropset already clears trades on mainnet today by
          routing through aggregators, and what you just saw is how we bootstrap
          the brand-new markets with the eCLOB.
        </Notes>
      </Slide>

      {/* 7 — Why this will fail */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Why this will fail</Eyebrow>
          <Statement>Why won’t this work? Arc, Tempo, Canton.</Statement>
          <CompetitorWall />
        </FlexBox>
        <Notes>
          The honest risk: everyone wants onchain settlement. Arc and Tempo are
          building payment-and-settlement rails, and Canton is doing regulated
          onchain markets. Any of them could decide FX is theirs.
        </Notes>
      </Slide>

      {/* 8 — Why it will work */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Why it will work</Eyebrow>
          <Statement fontSize="52px">
            They’re private or walled — and the big apps aren’t focused on FX.
          </Statement>
          <OpenVsClosed />
        </FlexBox>
        <Notes>
          But those are private, permissioned, or walled gardens. And big Solana
          apps like Jupiter aren’t focused on FX — it’s a smaller market today,
          so it’s a classic innovator’s dilemma: only a small, focused team goes
          after it now. Dropset is the open, neutral, composable venue — anyone
          can quote, anyone can trade, any app can integrate — and we’re beating
          everyone to it.
        </Notes>
      </Slide>

      {/* 9 — How we grow */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="52px">
            We bootstrap the liquidity ourselves — like Hyperliquid.
          </Statement>
          <GrowthCurve />
        </FlexBox>
        <Notes>
          We seed the markets ourselves the way Hyperliquid did — through a
          vault others can top off with inventory — and we help stablecoin
          issuers land their first real trades on mainnet. Colosseum partners
          like Altitude and Cargobill already need to source FX onchain —
          that’s our first demand.
        </Notes>
      </Slide>

      {/* 10 — Team & close */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Team</Eyebrow>
          <Statement>Built by people who’ve built exchanges.</Statement>
          <FlexBox margin="40px 0 0 0" justifyContent="center">
            <Portrait
              src="/remote/team-alex.png"
              name="Alex"
              role="Product · exchange design"
              size={148}
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy"
              role="Operations · stablecoin rails"
              size={132}
            />
          </FlexBox>
        </FlexBox>
        <Notes>
          I’ve built two onchain exchanges already, including an order book
          — I authored Econia on Aptos, which cleared around five hundred
          million in volume, and wrote the Solana Opcode Guide, the playbook for
          squeezing performance out of Solana programs. Judy owns operations
          end-to-end — banking, the stablecoin providers, onramps, and
          accounting. Dropset — Forex on Solana.
        </Notes>
      </Slide>
    </Deck>
  );
}
