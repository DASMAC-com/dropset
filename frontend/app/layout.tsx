// cspell:word darkreader
import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import { Header } from "@/components/Header";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "dropset",
  description: "dropset gamma prototype",
  // darkreader-lock: tells the Dark Reader extension to skip color
  // transforms on this page. The app is already dark-themed, and Dark
  // Reader's SVG attribute injection causes hydration mismatches.
  other: {
    "darkreader-lock": "",
  },
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    // suppressHydrationWarning: browser extensions like Dark Reader and
    // Grammarly inject attributes on <html>/<body> before React hydrates,
    // causing benign mismatches. Suppressing here is the canonical Next.js
    // workaround for these extension-induced diffs.
    <html
      lang="en"
      className={`${geistSans.variable} ${geistMono.variable} h-full antialiased`}
      suppressHydrationWarning
    >
      <body className="flex min-h-full flex-col" suppressHydrationWarning>
        <Header />
        <main className="flex-1">{children}</main>
      </body>
    </html>
  );
}
