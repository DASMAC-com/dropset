import { describe, expect, it } from "vitest";
import {
  committedIconFor,
  listCommitted,
  readTokens,
  URL_PREFIX,
} from "./token-icons-shared.mjs";

// Resolving a symbol to its committed file is the join that replaced a
// network fetch, so it is now the only thing standing between a listed
// currency and a missing icon. The extension is not predictable from the
// symbol — it records what upstream actually served, and differs per token
// (USDC is a .png, USDT and IDRX are .svg) — so this cannot be a composed
// path and has to be a directory match.
describe("committedIconFor", () => {
  const entries = ["USDC.png", "USDT.svg", "EUROP.png", "USD1.png"];

  it("matches on the stem, whatever the extension", () => {
    expect(committedIconFor("USDC", entries)).toEqual({
      filename: "USDC.png",
      path: expect.stringContaining("brand-assets/token-icons/USDC.png"),
    });
    expect(committedIconFor("USDT", entries)?.filename).toBe("USDT.svg");
  });

  it("returns undefined for a symbol with nothing committed", () => {
    expect(committedIconFor("AUDD", entries)).toBeUndefined();
  });

  // The whole-name comparison matters: a prefix match would resolve USD to
  // USDC.png and quietly ship the wrong artwork. USD1 sitting beside USDC in
  // the fixture is the case that makes this non-theoretical.
  it("does not match a symbol that is merely a prefix of a filename", () => {
    expect(committedIconFor("USD", entries)).toBeUndefined();
    expect(committedIconFor("EUR", entries)).toBeUndefined();
  });

  // An upstream format change leaves the old file behind unless someone
  // deletes it, and both would be copied into public/. Which one the manifest
  // named would then depend on directory order, so this throws rather than
  // picking. Reported loudly because the symptom otherwise is artwork that
  // changes on an unrelated rebuild.
  it("throws when a symbol has two committed icons", () => {
    expect(() => committedIconFor("USDC", ["USDC.png", "USDC.svg"])).toThrow(
      /USDC has 2 committed icons/,
    );
  });

  // A dotless entry is compared whole. Stripping "everything before the last
  // dot" on a name with no dot drops its final character, which would make a
  // stray `USDCx` match the symbol USDC.
  it("compares a dotless entry whole rather than dropping a character", () => {
    expect(committedIconFor("USDC", ["USDCx"])).toBeUndefined();
    expect(committedIconFor("USDCx", ["USDCx"])?.filename).toBe("USDCx");
  });
});

// The condition the REQUIRED `Frontend` job gates on, asserted here so it fails
// in `pnpm test` before it fails in CI. The currencies suite cannot substitute:
// its manifest assertion is a `.some(...)` (one populated entry passes) and its
// per-symbol assertion resolves through the remote-URL fallback, so a currency
// added with no committed icon leaves that suite green. Until this existed, the
// only verification of the gate was one hand-run mutation CI cannot repeat.
describe("committed icon coverage", () => {
  it("has a committed icon for every listed currency", () => {
    const entries = listCommitted();
    const missing = readTokens()
      .filter((token) => !committedIconFor(token.symbol, entries))
      .map((token) => token.symbol);
    expect(missing).toEqual([]);
  });

  // Guards the coupling that has no other enforcement: copy-brand-assets.mjs
  // serves the committed directory under its own basename, so a rename of
  // ICON_DIR silently 404s every icon while leaving the manifest strings — the
  // only thing the currencies suite checks — perfectly well-formed.
  it("derives the served prefix from the committed directory name", () => {
    expect(URL_PREFIX).toBe("/token-icons");
  });
});
