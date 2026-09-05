import { describe, expect, it } from "vitest";
import {
  ALL_STABLECOINS,
  tokenIconFallbackUrl,
  tokenIconUrl,
} from "./currencies";

// Pins the two-source icon resolution: the build-time mirror is preferred, and
// the issuer's canonical URL stays reachable behind it so a render-side error
// handler has somewhere to go. The overlay used to overwrite the canonical URL
// in place, which left a symbol whose mirrored file was broken with no recovery
// path at all — the browser rendered nothing, silently.
describe("token icon resolution", () => {
  const symbols = ALL_STABLECOINS.map((s) => s.symbol);

  it("resolves an icon for every supported stablecoin", () => {
    expect(symbols.length).toBeGreaterThan(0);
    for (const symbol of symbols) {
      expect(tokenIconUrl(symbol), `no icon for ${symbol}`).not.toEqual("");
    }
  });

  // Whichever source wins, the two must never be the same URL — a fallback
  // that points back at the thing that just failed is not a fallback.
  it("keeps the canonical URL distinct from the mirror", () => {
    // Without this the loop below is vacuous: an empty manifest makes every
    // fallback "", the body never runs, and the test passes asserting nothing.
    //
    // This assertion is now hermetic, which it previously was not. The
    // manifest is built from the icons committed under
    // brand-assets/token-icons/ with no network access, so an offline or
    // proxied install produces the same populated manifest as any other. An
    // empty one therefore means committed files are missing — not that the
    // install could not reach an issuer CDN, which is what it used to mean
    // and what the old message said.
    expect(
      symbols.some((s) => tokenIconFallbackUrl(s) !== ""),
      "icon-manifest.gen.json is empty — run " +
        "`pnpm --filter dropset-frontend build-token-manifest`; if it stays " +
        "empty, brand-assets/token-icons/ is missing its committed icons",
    ).toBe(true);
    for (const symbol of symbols) {
      const fallback = tokenIconFallbackUrl(symbol);
      if (fallback) {
        expect(fallback).not.toEqual(tokenIconUrl(symbol));
        // A mirrored entry serves from our own origin; the fallback is remote.
        expect(tokenIconUrl(symbol).startsWith("/token-icons/")).toBe(true);
        expect(fallback.startsWith("http")).toBe(true);
      }
    }
  });

  // An unknown symbol must resolve to "" rather than to a broken URL, so
  // TokenIcon renders its sized placeholder instead of asking the browser to
  // re-request the current page as an image.
  it("returns empty strings for an unknown symbol", () => {
    expect(tokenIconUrl("NOT_A_TOKEN")).toEqual("");
    expect(tokenIconFallbackUrl("NOT_A_TOKEN")).toEqual("");
  });
});
