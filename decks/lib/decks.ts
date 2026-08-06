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
  /**
   * ISO date the deck is **presented**, shown on its card.
   *
   * Deliberately not "last revised", which is what this used to be: a
   * revision date is stale the moment anyone edits a slide and forgets to
   * touch it, and a wrong date is worse than no date — it invites the reader
   * to wonder whether the deck is current. An event date is fixed, so it
   * needs no maintenance and answers the question the reader actually has.
   */
  presented: string;
};

export const decks: Deck[] = [
  {
    route: "/demo-v1",
    title: "Colosseum Cohort 5 Demo Day",
    subtitle: "The ~2-minute pitch: the gap, what's live today, and the eCLOB.",
    presented: "2026-08-26",
  },
];
