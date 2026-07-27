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
 * The competitors, as a wall closing in (page 5). Each is captioned with the
 * chain the presenter names out loud, which isn't always what its mark says —
 * Arc is Circle's, so Circle's logo is what an audience recognizes.
 */
const COMPETITORS = [
  { name: "Arc", src: "/remote/logo-circle.svg" },
  { name: "Tempo", src: "/remote/logo-tempo.svg" },
  { name: "Canton", src: "/remote/logo-canton.svg" },
];

const CompetitorWall = () => (
  <FlexBox margin="40px 0 0 0" justifyContent="center" alignItems="flex-start">
    {COMPETITORS.map(({ name, src }) => (
      <Box key={name} margin="0 12px">
        <Box
          border={`1px solid ${colors.sell}`}
          borderRadius="10px"
          padding="22px 30px"
        >
          {/* Height-matched, not width-matched: these logos have different
              aspect ratios, so a shared width would make one read twice the
              size of another. */}
          <Image src={src} height={48} alt={name} />
        </Box>
        <Text
          color={colors.sell}
          fontFamily="monospace"
          fontSize="24px"
          margin="10px 0 0 0"
        >
          {name}
        </Text>
      </Box>
    ))}
  </FlexBox>
);

/**
 * One open venue against a row of closed ones (page 6) — the visual answer to
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
 * The people we've talked to about sourcing liquidity — the payments companies
 * that need FX, and the issuers who need their currency to trade (page 7).
 *
 * Each entry renders its logo once one is listed in `remote-assets.json` (add
 * the URL there, then set `src` to the mirrored path); until then it renders as
 * type, so the page is never waiting on an asset to be presentable.
 */
const PARTNERS: { name: string; src?: string }[] = [
  { name: "Altitude" },
  { name: "Cargobill" },
  { name: "AUDD" },
  { name: "CADC" },
];

const PartnerRow = () => (
  <FlexBox alignItems="center" justifyContent="center" margin="10px 0 0 0">
    {PARTNERS.map(({ name, src }) => (
      <Box key={name} margin="0 18px">
        {src ? (
          <Image src={src} width={124} alt={name} />
        ) : (
          <Text color="quaternary" fontSize="28px" margin="0">
            {name}
          </Text>
        )}
      </Box>
    ))}
  </FlexBox>
);

/**
 * Depth growing from a seeded book (page 7), over the partners and issuers that
 * bring the flow. An inline SVG keeps the curve crisp at projector size and
 * needs no asset pipeline.
 */
const GrowthCurve = () => (
  <Box margin="30px 0 0 0">
    <svg
      width="720"
      height="200"
      viewBox="0 0 720 200"
      role="img"
      aria-label="Order-book depth growing over time"
    >
      <path
        d="M0 190 C 210 185, 380 152, 500 96 C 590 55, 660 24, 720 10 L 720 200 L 0 200 Z"
        fill={colors.accent}
        opacity="0.14"
      />
      <path
        d="M0 190 C 210 185, 380 152, 500 96 C 590 55, 660 24, 720 10"
        fill="none"
        stroke={colors.accent}
        strokeWidth="4"
      />
    </svg>
    <Text color="quaternary" fontSize="26px" margin="6px 0 0 0">
      We bootstrap vaults for a public liquidity flywheel
    </Text>
    <PartnerRow />
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
}: {
  src: string;
  name: string;
  role: string;
}) => (
  <Box margin="0 32px">
    <Image src={src} width={140} height={140} alt={name} />
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
          <Statement>This already works — today, on mainnet.</Statement>
          <Screenshot
            src="/screens/mainnet-globe.png"
            width={430}
            alt="The Dropset globe, currencies pinned to the countries that issue them"
          />
          <DemoBadge network="mainnet" />
        </FlexBox>
        <Notes>
          Start with what already works. Dropset is live on mainnet today,
          clearing real trades by routing FX through aggregators — pick the
          currency you want on the globe and the swap settles. [Play the mainnet
          demo video.]
        </Notes>
      </Slide>

      {/* 4 — The eCLOB */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>The eCLOB</Eyebrow>
          <Statement fontSize="48px">
            And we’re just getting started: order-book depth, propAMM-cheap
            quotes.
          </Statement>
          <FlexBox alignItems="center" justifyContent="center">
            <Box margin="0 18px 0 0">
              <Screenshot
                src="/screens/maker-tui.png"
                width={370}
                alt="The maker control panel: seven FX markets and a live EURC book"
                caption="The maker’s control panel"
              />
            </Box>
            <Box margin="0 0 0 18px">
              <Screenshot
                src="/screens/compute-units.png"
                width={370}
                alt="Compute units per instruction: a reprice costs 47, a reshape 491"
                caption="Reprice: 47 CU · reshape: 491 CU"
              />
            </Box>
          </FlexBox>
          <DemoBadge network="localnet" />
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
          <Statement>Why won’t this work? Arc, Tempo, Canton.</Statement>
          <CompetitorWall />
        </FlexBox>
        <Notes>
          The honest risk: everyone wants onchain settlement. Arc and Tempo are
          building payment-and-settlement rails, and Canton is doing regulated
          onchain markets. Any of them could decide FX is theirs.
        </Notes>
      </Slide>

      {/* 6 — Why it will work */}
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

      {/* 7 — How we grow */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>How we grow</Eyebrow>
          <Statement fontSize="52px">
            We bootstrap the liquidity ourselves — like Hyperliquid.
          </Statement>
          <GrowthCurve />
        </FlexBox>
        <Notes>
          We seed the markets ourselves the way Hyperliquid did — we bootstrap
          the vaults, and anyone can top them off with inventory, so the
          flywheel is public rather than ours alone. And we help stablecoin
          issuers land their first real trades on mainnet. We’ve talked with all
          of these about sourcing liquidity: Colosseum partners like Altitude
          and Cargobill need to buy FX onchain, and issuers like AUDD and CADC
          need their currency to actually trade.
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
            />
            <Portrait
              src="/remote/team-judy.png"
              name="Judy"
              role="Operations · stablecoin rails"
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
