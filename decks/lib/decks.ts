/**
 * The deck registry. Each deck is a route under `decks.dropset.io/`; the
 * landing page renders this list. Route names are public-facing (e.g.
 * `/demo-v1`) — never internal ticket ids, which must not leak into
 * shareable URLs. Adding a deck is: a new route folder + an entry here.
 */
export type Deck = {
  route: string;
  title: string;
  subtitle: string;
  /** ISO date the deck was last revised, shown on its card. */
  updated: string;
};

export const decks: Deck[] = [
  {
    route: "/demo-v1",
    title: "Colosseum Demo Day",
    subtitle: "Forex on Solana: vision, mainnet and localnet demos.",
    updated: "2026-07-27",
  },
];
