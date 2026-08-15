// @ts-check

/**
 * The deck registry. Each deck is a route under `decks.dropset.io/`; the
 * landing page renders this list. Route names are public-facing (e.g.
 * `/demo-v1`) — never internal ticket ids, which must not leak into
 * shareable URLs. Adding a deck is: a new route folder + an entry here.
 *
 * Plain JavaScript rather than TypeScript because both readers have to reach
 * it unaided: the landing page imports it through the bundler, while
 * `pnpm run export` is a bare Node script with no TypeScript loader in front
 * of it. The JSDoc below carries the same types back to the TypeScript side,
 * which picks them up through `allowJs`.
 */

/**
 * @typedef {object} Deck
 * @property {string} route The deck's public path, e.g. `/demo-v1`.
 * @property {string} title
 * @property {string} subtitle
 * @property {string} presented
 *   ISO date the deck is **presented**, shown on its card.
 *
 *   Deliberately not "last revised", which is what this used to be: a
 *   revision date is stale the moment anyone edits a slide and forgets to
 *   touch it, and a wrong date is worse than no date — it invites the reader
 *   to wonder whether the deck is current. An event date is fixed, so it
 *   needs no maintenance and answers the question the reader actually has.
 * @property {number} pages
 *   How many pages the deck has.
 *
 *   The export pipeline screenshots pages by index, and a headless browser
 *   given an out-of-range `slideIndex` renders the last page again rather than
 *   failing — so a count that is too high yields silent duplicates and one too
 *   low silently truncates the deck. Neither is visible until someone opens
 *   the `.pptx`. Declaring it here keeps it next to the route it describes.
 */

/** @type {Deck[]} */
export const decks = [
  {
    route: "/demo-v1",
    title: "Colosseum Cohort 5 Demo Day",
    subtitle:
      "The ~2-minute pitch: the gap, what's live today, the eCLOB, and why FX is next.",
    presented: "2026-08-26",
    pages: 11,
  },
];
