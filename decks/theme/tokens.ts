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
  /** The deck backdrop — see the `deckTheme` note on why it isn't `background`. */
  backdrop: "#000000",
} as const;

/**
 * The DASMAC brand faces (Kargil Studios design system): Inter as the primary
 * family, Space Mono as the mono/tag face — matching the product website, which
 * types in Space Mono. The variables are declared by `next/font/google` in
 * `app/layout.tsx`; the fallbacks matter for the print path, where a font that
 * hasn't loaded yet would otherwise reflow the slide it's measuring.
 */
const sansStack = "var(--font-inter), system-ui, sans-serif";
const monoStack = "var(--font-space-mono), ui-monospace, monospace";

/**
 * Exactly 16:9 — a firm requirement, since slides are printed and dropped into
 * a 16:9 Google Slides canvas for the accelerator's combined meta-deck, and any
 * other ratio letterboxes or crops there.
 *
 * Spectacle's own default is 1366×768, which is 1.7786 rather than 1.7778 — off
 * by a fraction of a percent, invisible on screen but enough to leave a hairline
 * band after the print-and-import round trip. Stating 1920×1080 makes the ratio
 * exact and the design space a familiar one. Spectacle scales this box to
 * whatever it's displayed on, so the numbers are a coordinate system, not a
 * resolution cap.
 */
export const DECK_SIZE = { width: 1920, height: 1080 } as const;

/**
 * Spectacle consumes a theme via the `<Deck theme={...}>` prop. The color
 * keys map onto Spectacle's semantic slots: `primary` is body text,
 * `secondary` is the accent used by headings/links, `tertiary` is the deck
 * backdrop.
 *
 * The backdrop is pure black rather than the frontend's near-black `background`
 * (#0a0a0a), which is the one deliberate break from that mirror. The brand
 * wordmark ships as an opaque PNG on solid black, so on a near-black slide it
 * draws a faintly lighter rectangle around itself — most visible in the footer,
 * where a blend mode can't reach past the footer's own stacking context. Making
 * the backdrop the same black the asset carries removes the seam everywhere at
 * once, and is indistinguishable from #0a0a0a on a projector.
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
  /**
   * Spectacle takes the native slide box from the **theme**, not a `<Deck>`
   * prop, and one `size` here drives all three render paths — the on-screen
   * aspect-ratio fitter, the slide overview, and print. That last one is why
   * this is stated rather than left to default: print is how slides reach the
   * accelerator's Google Slides meta-deck.
   */
  size: DECK_SIZE,
  colors: {
    primary: colors.foreground,
    secondary: colors.accent,
    tertiary: colors.backdrop,
    quaternary: colors.mutedFg,
    quinary: colors.border,
  },
  backdropStyle: {
    position: "fixed",
    top: 0,
    left: 0,
    width: "100vw",
    height: "100vh",
    backgroundColor: colors.backdrop,
  },
  fonts: {
    header: sansStack,
    text: sansStack,
    monospace: monoStack,
  },
  /**
   * Sized for the 1920×1080 design space in `DECK_SIZE`, ~1.4× the values that
   * suited Spectacle's default 1366-wide box. Every explicit size in a deck is
   * in these same slide units, so the two have to be read together — a value
   * copied from an older 1366-era slide reads a third too small.
   */
  fontSizes: {
    h1: "96px",
    h2: "68px",
    h3: "48px",
    text: "36px",
    monospace: "28px",
  },
  space: [16, 24, 32],
};
