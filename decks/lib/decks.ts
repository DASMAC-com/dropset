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
    title: "Demo-day pitch",
    subtitle:
      "The 2-minute accelerator pitch — onchain Forex on Solana, built around a live demo.",
    updated: "2026-08-26",
  },
];
