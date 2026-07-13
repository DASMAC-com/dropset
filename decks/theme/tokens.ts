/**
 * Dropset design tokens, mirrored from the frontend surface
 * (frontend/app/globals.css) and re-shaped into a Spectacle deck theme.
 *
 * Kept as plain constants here — rather than importing frontend's CSS — so
 * the decks package stays a standalone Vercel build with no cross-package
 * runtime coupling. The values are the single source of visual truth for
 * every deck; the raw `colors` map is also exported for inline use in JSX.
 */
export const colors = {
  background: "#0a0a0a",
  foreground: "#ededed",
  muted: "#1a1a1a",
  mutedFg: "#a3a3a3",
  border: "#262626",
  accent: "#60a5fa",
  accentHover: "#93bbfd",
  buy: "#10b981",
  sell: "#ef4444",
  brand: "#0044ff",
} as const;

const sansStack = "var(--font-geist-sans), system-ui, sans-serif";
const monoStack = "var(--font-geist-mono), ui-monospace, monospace";

/**
 * Spectacle consumes a theme via the `<Deck theme={...}>` prop. The color
 * keys map onto Spectacle's semantic slots: `primary` is body text,
 * `secondary` is the accent used by headings/links, `tertiary` is the deck
 * backdrop.
 *
 * `backdropStyle` must carry the full-viewport sizing itself: a theme-level
 * `backdropStyle` *replaces* Spectacle's default backdrop object wholesale,
 * and that default is what pins the backdrop to `position: fixed` at
 * `100vw × 100vh`. Spectacle's aspect-ratio fitter scales and centers each
 * slide by measuring this backdrop, so if it collapses out of the viewport
 * (as a bare `{ backgroundColor }` override does) the slide renders small and
 * top-anchored on a large monitor instead of centered. We keep the fitter's
 * transform-origin centering — don't add flex centering here, which would
 * double-offset the already-transformed slide.
 */
export const deckTheme = {
  colors: {
    primary: colors.foreground,
    secondary: colors.accent,
    tertiary: colors.background,
    quaternary: colors.mutedFg,
    quinary: colors.border,
  },
  backdropStyle: {
    position: "fixed",
    top: 0,
    left: 0,
    width: "100vw",
    height: "100vh",
    backgroundColor: colors.background,
  },
  fonts: {
    header: sansStack,
    text: sansStack,
    monospace: monoStack,
  },
  fontSizes: {
    h1: "68px",
    h2: "48px",
    h3: "34px",
    text: "26px",
    monospace: "20px",
  },
  space: [16, 24, 32],
};
