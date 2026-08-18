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
