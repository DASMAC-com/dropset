import type { Metadata, Viewport } from "next";
import { GeistMono } from "geist/font/mono";
import { GeistSans } from "geist/font/sans";
import "./globals.css";

const title = "Dropset Decks";
const description = "Presentation decks for Dropset — forex on Solana.";

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
      className={`${GeistSans.variable} ${GeistMono.variable} antialiased`}
      suppressHydrationWarning
    >
      <body suppressHydrationWarning>{children}</body>
    </html>
  );
}
