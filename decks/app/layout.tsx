import type { Metadata, Viewport } from "next";
import { Inter, Space_Mono } from "next/font/google";
import "./globals.css";

/**
 * The DASMAC brand faces, per the Kargil Studios design system: Inter as the
 * primary family, Space Mono as the mono/tag face. Space Mono is what the
 * product website types in, so the deck matches the site rather than the
 * earlier JetBrains Mono note on the DASMAC page.
 *
 * Both come from `next/font/google`, mirroring `frontend/app/layout.tsx` — the
 * files are fetched and self-hosted at build time, so there is nothing to
 * commit. Space Mono is not a variable font, so its weights are enumerated;
 * 700 is what the deck's monospace eyebrows and tags use for emphasis.
 */
const inter = Inter({ subsets: ["latin"], variable: "--font-inter" });
const spaceMono = Space_Mono({
  subsets: ["latin"],
  variable: "--font-space-mono",
  weight: ["400", "700"],
});

const title = "Dropset Decks";
const description =
  "Presentation decks for Dropset — where currency trades onchain.";

export const viewport: Viewport = {
  themeColor: "#0a0a0a",
};

export const metadata: Metadata = {
  title,
  description,
  icons: {
    // Stroked favicon variant, mirroring the frontend. Safari's undocumented
    // low-contrast heuristic adds a white "chip" behind the brand blue
    // (#0044FF) favicon; the outline clears it. See the fuller rationale in
    // frontend/app/layout.tsx.
    icon: { url: "/favicon-with-stroke.svg", type: "image/svg+xml" },
    apple: "/favicon-with-stroke.svg",
  },
  // Decks are internal/shareable-link material, not something to index.
  robots: { index: false, follow: false },
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html
      lang="en"
      className={`${inter.variable} ${spaceMono.variable} antialiased`}
      suppressHydrationWarning
    >
      <body suppressHydrationWarning>{children}</body>
    </html>
  );
}
