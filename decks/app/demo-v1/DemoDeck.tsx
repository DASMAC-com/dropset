"use client";

// cspell:word Econia
// cspell:word emojicoin
// cspell:word Aptos

import {
  Box,
  Deck,
  FlexBox,
  Heading,
  Image,
  ListItem,
  Notes,
  Progress,
  Slide,
  Text,
  UnorderedList,
} from "spectacle";
import { colors, deckTheme } from "@/theme/tokens";

/**
 * The demo-day pitch deck — a ~2-minute accelerator pitch built around a
 * live product demo. Slides are minimal backdrops the presenter talks over;
 * the full spoken script lives in each slide's `<Notes>` (presenter mode,
 * `p`), never on the slide itself. Route name is public-facing (`/demo-v1`);
 * internal ticket ids never appear here or in the URL.
 *
 * Structure follows the accelerator's seven-point pitch: one-liner → why
 * now → the eCLOB → live on mainnet [demo] → flash liquidity [demo] → next
 * steps → team. Timing guide: slides 1–3 ≈ 40s, demo (4–5) ≈ 55s,
 * slides 6–7 ≈ 25s.
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

// A live-demo cue the presenter reads as "run the demo here".
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

export default function DemoDeck() {
  return (
    <Deck theme={deckTheme} template={template}>
      {/* 1 — Title / one-liner */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Box margin="0 0 28px 0">
            <Image src="/dropset-wordmark.png" width={260} />
          </Box>
          <Heading fontSize="72px" margin="0">
            Onchain Forex, on Solana
          </Heading>
          <Text color="quaternary" fontSize="30px" margin="20px 0 0 0">
            Currencies, exchanged at scale.
          </Text>
        </FlexBox>
        <Notes>
          DASMAC is building Dropset — an onchain Forex platform on Solana for
          the open, efficient exchange of the world&apos;s currencies at scale.
        </Notes>
      </Slide>

      {/* 2 — Why now, why big */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Why now, why big</Eyebrow>
          <UnorderedList fontSize="44px">
            <ListItem>FX: $9T daily, 24/5</ListItem>
            <ListItem>~14 currencies on Solana</ListItem>
            <ListItem>No liquid onchain home</ListItem>
          </UnorderedList>
        </FlexBox>
        <Notes>
          Foreign exchange is the biggest market on earth — over nine trillion
          dollars a day, trading around the clock. But the top FX stablecoins
          have no liquid home on Solana, and that market is only just opening:
          around fourteen currencies are represented on-chain now, with the
          euro driving most of the volume. Settle FX through Solana and you get
          atomic settlement and near-instant on- and off-ramps.
        </Notes>
      </Slide>

      {/* 3 — The eCLOB */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>The eCLOB</Eyebrow>
          <UnorderedList fontSize="44px">
            <ListItem>Order-book depth</ListItem>
            <ListItem>propAMM-cheap quotes</ListItem>
            <ListItem>Faster maker onboarding</ListItem>
          </UnorderedList>
        </FlexBox>
        <Notes>
          Our edge is a novel exchange design, the eCLOB. You get the liquidity
          guarantees of a central limit order book, but with quote updates as
          cheap as a proportional AMM. That combination lets us source
          liquidity where there&apos;s almost none today, and onboard market
          makers far faster than a traditional book.
        </Notes>
      </Slide>

      {/* 4 — Live on mainnet [demo] */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Live on mainnet</Eyebrow>
          <Heading fontSize="60px" margin="0">
            Live today, clearing trades.
          </Heading>
          <DemoCue>EURC/USDC mainnet swap</DemoCue>
        </FlexBox>
        <Notes>
          And it&apos;s not a prototype — Dropset is live on mainnet today,
          clearing real trades. Here I&apos;ll execute a real EURC/USDC swap on
          the live frontend. [Run the swap.] That just settled on mainnet.
        </Notes>
      </Slide>

      {/* 5 — Flash liquidity (localnet) [demo] */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Flash liquidity · localnet</Eyebrow>
          <UnorderedList fontSize="44px">
            <ListItem>Empty → makers on</ListItem>
            <ListItem>Book fills live</ListItem>
            <ListItem>eCLOB fills the size</ListItem>
          </UnorderedList>
          <DemoCue>empty market → real depth</DemoCue>
        </FlexBox>
        <Notes>
          To show how a book comes to life, here&apos;s the same stack on
          localnet, starting from an empty market. I turn the maker bots on and
          you watch top-of-book fill in live. Then I execute against real eCLOB
          depth — the eCLOB-only route — and it fills the size. [Optional: from
          the control TUI I can reshape the ladder or reprice the whole book in
          a single instruction.]
        </Notes>
      </Slide>

      {/* 6 — Next: win EURC/USDC */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Next: win EURC/USDC</Eyebrow>
          <UnorderedList fontSize="44px">
            <ListItem>Profiling Orca depth</ListItem>
            <ListItem>Undercut spread + size</ListItem>
            <ListItem>Own the flagship pair</ListItem>
          </UnorderedList>
        </FlexBox>
        <Notes>
          Next, we go after the flagship pair. We&apos;re profiling Orca&apos;s
          liquidity on EURC/USDC so we can undercut it on both spread and size,
          and own that pair.
        </Notes>
      </Slide>

      {/* 7 — Team / close */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Eyebrow>Team</Eyebrow>
          <UnorderedList fontSize="40px">
            <ListItem>Authored Econia ($500M)</ListItem>
            <ListItem>Co-authored Solana Opcode Guide</ListItem>
            <ListItem>Founder stays on product</ListItem>
          </UnorderedList>
          <Box margin="44px 0 0 0">
            <Image src="/dropset-wordmark.png" width={200} />
          </Box>
        </FlexBox>
        <Notes>
          On the team: we&apos;ve built exchange infrastructure before. I
          authored Econia, the onchain order book on Aptos that cleared around
          five hundred million dollars in volume, and co-authored emojicoin.fun
          and the Solana Opcode Guide. We&apos;ve brought on an operations lead
          so I can stay focused on product. Dropset — onchain Forex, on Solana.
        </Notes>
      </Slide>
    </Deck>
  );
}
