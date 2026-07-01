"use client";

import {
  Appear,
  Box,
  Deck,
  FlexBox,
  Heading,
  Image,
  ListItem,
  Notes,
  OrderedList,
  Progress,
  Slide,
  Table,
  TableBody,
  TableCell,
  TableHeader,
  TableRow,
  Text,
  UnorderedList,
} from "spectacle";
import { colors, deckTheme } from "@/theme/tokens";

/**
 * "Multi-market FX stablecoin liquidity" — the accelerator demo-day deck.
 * Source context: the ENG-634 demo spec (pitch, demo arc, money shot,
 * roster, authority model). Route name is public-facing (`/demo-v1`); the
 * internal ticket id never appears here or in the URL.
 */

// The seven-market roster (liquidity query, 2026-06-29). Mint columns are
// dropped for legibility; the `$10k slip` column is the real depth read.
const roster = [
  { sym: "EURC", ccy: "EUR", liq: "$596k", slip: "0.03%", thin: false },
  { sym: "VCHF", ccy: "CHF", liq: "$121k", slip: "0.25%", thin: false },
  { sym: "TGBP", ccy: "GBP", liq: "$108k", slip: "0.11%", thin: false },
  { sym: "ZARP", ccy: "ZAR", liq: "$36k", slip: "2.38%", thin: true },
  { sym: "MXNe", ccy: "MXN", liq: "$5.5k", slip: "0.26%", thin: true },
  { sym: "XSGD", ccy: "SGD", liq: "$5.4k", slip: "no route @ $10k", thin: true },
  { sym: "IDRX", ccy: "IDR", liq: "$4.2k", slip: "6.34%", thin: true },
];

const template = () => (
  <FlexBox
    justifyContent="space-between"
    position="absolute"
    bottom={0}
    width={1}
    zIndex={1}
  >
    <Box padding="0 1.25em">
      <Image src="/watermark.svg" width={110} />
    </Box>
    <Box padding="0 1.25em">
      <Progress color={colors.accent} size={8} />
    </Box>
  </FlexBox>
);

export default function DemoDeck() {
  return (
    <Deck theme={deckTheme} template={template}>
      {/* 1 — Title */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Text
            color="secondary"
            fontFamily="monospace"
            fontSize="22px"
            margin="0 0 12px 0"
          >
            Dropset · Forex on Solana
          </Text>
          <Heading fontSize="72px" margin="0">
            Multi-market FX liquidity
          </Heading>
          <Text color="quaternary" fontSize="30px" margin="16px 0 0 0">
            One eCLOB + maker stack flashing credible top-of-book across seven
            FX stablecoins — all routed through one SDK.
          </Text>
        </FlexBox>
        <Notes>
          Breadth-of-integration story, not a depth-of-strategy quant story.
          The demo IS the mainnet MVP.
        </Notes>
      </Slide>

      {/* 2 — The problem */}
      <Slide>
        <Heading color="secondary" fontSize="34px">
          The pitch
        </Heading>
        <Heading fontSize="60px" margin="12px 0 0 0">
          The top FX stablecoins have no liquid home on Solana.
        </Heading>
        <Text color="quaternary" fontSize="28px" margin="32px 0 0 0">
          Dropset's eCLOB + maker stack can quote all of them at once, in
          parallel, from a single integration surface.
        </Text>
        <Notes>Orca/Coinbase/Aerodrome benchmarks are parked in ENG-606.</Notes>
      </Slide>

      {/* 3 — The insight */}
      <Slide>
        <Heading color="secondary" fontSize="34px">
          Why this works
        </Heading>
        <Heading fontSize="56px" margin="12px 0 24px 0">
          $100 fills fine. <span style={{ color: colors.accent }}>Size</span>{" "}
          doesn't.
        </Heading>
        <UnorderedList fontSize="28px">
          <Appear>
            <ListItem>
              At $100, DFlow already fills all seven at ~$99.6–$99.97.
              Illiquidity doesn't bite at $100.
            </ListItem>
          </Appear>
          <Appear>
            <ListItem>
              It bites at <em>size</em>: four of seven (ZARP, MXNe, XSGD, IDRX)
              return <strong style={{ color: colors.sell }}>route not
              found</strong> at $10k–$50k — no fill exists anywhere on Solana.
            </ListItem>
          </Appear>
          <Appear>
            <ListItem>
              XSGD can't route <strong>$10k</strong>. That's the gap we fill.
            </ListItem>
          </Appear>
        </UnorderedList>
        <Notes>The $10k slip column (clean single-source DFlow) is the real depth read.</Notes>
      </Slide>

      {/* 4 — Demo arc */}
      <Slide>
        <Heading color="secondary" fontSize="34px">
          The demo, live
        </Heading>
        <OrderedList fontSize="26px">
          <Appear>
            <ListItem>Pick an FX stablecoin, attempt a swap — the market is empty.</ListItem>
          </Appear>
          <Appear>
            <ListItem>Flash liquidity: launch the maker bots, each quoting $100 top-of-book.</ListItem>
          </Appear>
          <Appear>
            <ListItem>Back to the frontend — the book fills in live; execute against real depth (eCLOB-only route).</ListItem>
          </Appear>
          <Appear>
            <ListItem>Walk the TUI: book-selector, per-bot start/stop, taker toggle.</ListItem>
          </Appear>
          <Appear>
            <ListItem>Show the eCLOB model: reshape the ladder, then nudge the reference price.</ListItem>
          </Appear>
        </OrderedList>
        <Notes>Every component is wired through the SDK — emphasize that.</Notes>
      </Slide>

      {/* 5 — The money shot */}
      <Slide backgroundColor={colors.muted}>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Text color="secondary" fontFamily="monospace" fontSize="22px">
            The money shot
          </Text>
          <Heading fontSize="60px" margin="8px 0 0 0">
            No route → we made one.
          </Heading>
          <Text fontSize="30px" color="quaternary" margin="28px 0 0 0">
            On a thin market (XSGD / ZARP), a ~$10k swap returns{" "}
            <strong style={{ color: colors.sell }}>no route at all</strong>{" "}
            beforehand — and fills afterward. The "Best route vs eCLOB-only"
            toggle makes the point that this depth is{" "}
            <strong style={{ color: colors.buy }}>ours</strong>.
          </Text>
        </FlexBox>
      </Slide>

      {/* 6 — eCLOB != AMM */}
      <Slide>
        <Heading color="secondary" fontSize="34px">
          eCLOB ≠ AMM
        </Heading>
        <Heading fontSize="48px" margin="12px 0 24px 0">
          Two instructions, two visual effects.
        </Heading>
        <FlexBox justifyContent="space-between" alignItems="flex-start">
          <Box width="46%">
            <Heading fontSize="30px" color="secondary">
              Reshape
            </Heading>
            <Text fontSize="24px" color="quaternary">
              Switch a market's liquidity profile — the ladder redistributes,
              spread widens or tightens, <em>peg fixed</em>.
            </Text>
          </Box>
          <Box width="46%">
            <Heading fontSize="30px" color="secondary">
              Reprice
            </Heading>
            <Text fontSize="24px" color="quaternary">
              Nudge the reference price — the <em>whole book shifts</em> as one.
            </Text>
          </Box>
        </FlexBox>
        <Text fontSize="24px" margin="28px 0 0 0">
          An AMM can do neither on demand.
        </Text>
      </Slide>

      {/* 7 — Roster */}
      <Slide>
        <Heading color="secondary" fontSize="30px" margin="0 0 12px 0">
          Seven markets · $100 top-of-book to seed
        </Heading>
        <Table fontSize="22px">
          <TableHeader>
            <TableRow>
              <TableCell>Sym</TableCell>
              <TableCell>Ccy</TableCell>
              <TableCell>Liquidity</TableCell>
              <TableCell>$10k slippage</TableCell>
            </TableRow>
          </TableHeader>
          <TableBody>
            {roster.map((r) => (
              <TableRow key={r.sym}>
                <TableCell>
                  <Text fontFamily="monospace" fontSize="22px" margin="0">
                    {r.sym}
                  </Text>
                </TableCell>
                <TableCell>{r.ccy}</TableCell>
                <TableCell>{r.liq}</TableCell>
                <TableCell>
                  <Text
                    fontSize="22px"
                    margin="0"
                    color={r.thin ? undefined : "quaternary"}
                    style={r.thin ? { color: colors.sell } : undefined}
                  >
                    {r.slip}
                  </Text>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
        <Notes>Source: Jupiter Tokens API + DFlow /quote, 2026-06-29. It's 7, not 8 — no clean 8th.</Notes>
      </Slide>

      {/* 8 — Architecture */}
      <Slide>
        <Heading color="secondary" fontSize="34px">
          How it's wired
        </Heading>
        <UnorderedList fontSize="26px">
          <ListItem>
            One <strong>leader</strong> key (cold) custodies all vaults.
          </ListItem>
          <ListItem>
            Seven delegated <strong>quote_authority</strong> hot keys — one per
            market. A hot key can only mis-quote its one market, never touch
            inventory.
          </ListItem>
          <ListItem>Quoting runs in parallel across markets on Sealevel.</ListItem>
          <ListItem>
            Frontend, TUI, and bots all speak to the chain through the{" "}
            <strong style={{ color: colors.accent }}>same SDK</strong>.
          </ListItem>
        </UnorderedList>
        <Notes>No cross-market batching: gas is per-signature ~$0.001, and 7 profiles don't fit the 1,232-byte tx limit.</Notes>
      </Slide>

      {/* 9 — Close */}
      <Slide>
        <FlexBox height="100%" flexDirection="column" justifyContent="center">
          <Heading fontSize="64px" margin="0">
            Liquidity from nothing.
          </Heading>
          <Text color="quaternary" fontSize="30px" margin="20px 0 0 0">
            Seven FX markets, one SDK, credible depth on demand — on mainnet.
          </Text>
          <Box margin="40px 0 0 0">
            <Image src="/watermark.svg" width={170} />
          </Box>
        </FlexBox>
      </Slide>
    </Deck>
  );
}
