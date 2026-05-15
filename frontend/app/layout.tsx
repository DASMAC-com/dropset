// cspell:word darkreader
import type { Metadata, Viewport } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import { Footer } from "@/components/Footer";
import { Header } from "@/components/Header";
import { TermsOfUseGate } from "@/components/TermsOfUseGate";
import { Providers } from "@/lib/providers";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

const title = "Dropset | Forex on Solana";
const description =
  "Open and efficient national currency exchange through the power of blockchain. Forex at the speed of Solana.";
const imageUrl = "/dropset-meta-main.png";
const imageAlt = "Dropset — Currency Exchange on Solana";
const images = [{ url: imageUrl, alt: imageAlt, width: 1200, height: 630 }];
const url = "https://dropset.io/";

const jsonLd = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "WebSite",
      "@id": `${url}#website`,
      url,
      name: "Dropset",
      description,
      publisher: { "@id": `${url}#organization` },
    },
    {
      "@type": "Organization",
      "@id": `${url}#organization`,
      name: "Dropset",
      url,
      logo: {
        "@type": "ImageObject",
        "@id": `${url}#logo`,
        url: `${url}dropset-wordmark.png`,
        width: 1000,
        height: 200,
        caption: "Dropset",
      },
      sameAs: ["https://x.com/__Dropset__"],
    },
  ],
};

export const viewport: Viewport = {
  themeColor: "#0a0a0a",
};

export const metadata: Metadata = {
  metadataBase: new URL(url),
  title,
  description,
  applicationName: "Dropset",
  appleWebApp: { title: "Dropset" },
  icons: {
    icon: "/favicon.png",
    apple: "/favicon.png",
  },
  openGraph: {
    type: "website",
    url,
    title,
    description,
    siteName: "Dropset",
    locale: "en",
    images,
  },
  twitter: {
    card: "summary_large_image",
    site: "dropset.io",
    creator: "@__Dropset__",
    title,
    description,
    images,
  },
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
        <script
          type="application/ld+json"
          // biome-ignore lint/security/noDangerouslySetInnerHtml: JSON.stringify of a static object — safe and the canonical Next.js pattern for JSON-LD.
          dangerouslySetInnerHTML={{ __html: JSON.stringify(jsonLd) }}
        />
        <Providers>
          <Header />
          <main className="flex-1">{children}</main>
          <Footer />
          <TermsOfUseGate />
        </Providers>
      </body>
    </html>
  );
}
