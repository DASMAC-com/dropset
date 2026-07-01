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
 * backdrop. `backdropStyle` pins the full-screen backdrop explicitly so it
 * never depends on Spectacle's token defaults.
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
