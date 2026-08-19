import type { WalletConnector } from "@solana/client";
import { describe, expect, it } from "vitest";
import {
  buildPickerWallets,
  KNOWN_WALLETS,
  walletIconFallbackUrl,
  walletIconUrl,
} from "./wallets";

// Only the three fields the picker's merge reads; the rest of the Wallet
// Standard surface is irrelevant to icon resolution.
const connector = (id: string, name: string, icon?: string): WalletConnector =>
  ({ id, name, icon }) as unknown as WalletConnector;

// Pins the two-source icon resolution: the build-time mirror is preferred, and
// the vendor's canonical URL stays reachable behind it so a render-side error
// handler has somewhere to go. The overlay used to overwrite the canonical URL
// in place, which left a wallet whose mirrored file was broken with no
// recovery path at all — the picker rendered an empty box, silently.
describe("wallet icon resolution", () => {
  const keys = KNOWN_WALLETS.map((w) => w.key);

  it("resolves an icon for every curated wallet", () => {
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) {
      expect(walletIconUrl(key), `no icon for ${key}`).not.toEqual("");
    }
  });

  // Whichever source wins, the two must never be the same URL — a fallback
  // that points back at the thing that just failed is not a fallback.
  it("keeps the canonical URL distinct from the mirror", () => {
    // This is the one assertion here that is not hermetic. `postinstall` runs
    // the fetch scripts WITHOUT --strict, so an offline or proxied install
    // still writes the manifest — just empty. The message matters more than
    // usual because the bare failure would read as a logic bug rather than as
    // "your install could not reach the vendor CDNs".
    expect(
      keys.some((k) => walletIconFallbackUrl(k) !== ""),
      "wallet-manifest.gen.json is empty — run `pnpm --dir frontend install` " +
        "with network access so the mirror is populated",
    ).toBe(true);
    for (const key of keys) {
      const fallback = walletIconFallbackUrl(key);
      if (fallback) {
        expect(fallback).not.toEqual(walletIconUrl(key));
        // A mirrored entry serves from our own origin; the fallback is remote.
        expect(walletIconUrl(key).startsWith("/wallet-icons/")).toBe(true);
        expect(fallback.startsWith("http")).toBe(true);
      }
    }
  });

  // An unknown key must resolve to "" rather than to a broken URL, so
  // WalletIcon renders its letter avatar instead of asking the browser to
  // re-request the current page as an image.
  it("returns empty strings for an unknown key", () => {
    expect(walletIconUrl("not_a_wallet")).toEqual("");
    expect(walletIconFallbackUrl("not_a_wallet")).toEqual("");
  });
});

// The connective tissue: whether a row carries a second source at all is
// decided here, not in the component.
describe("buildPickerWallets icon sources", () => {
  // Any curated entry will do — this exercises the merge, not a brand.
  const curated = KNOWN_WALLETS[0];
  if (!curated) throw new Error("wallets.json has no curated entries");
  const curatedKey = curated.key;

  it("gives an uninstalled curated wallet both sources", () => {
    const { notDetected } = buildPickerWallets([], false);
    const row = notDetected.find((w) => w.key === curatedKey);
    expect(row?.icon).toEqual(walletIconUrl(curatedKey));
    expect(row?.iconFallback).toEqual(walletIconFallbackUrl(curatedKey));
  });

  // A connector's icon is an inline data URI carrying its own bytes, so there
  // is no fetch that can fail and nothing to promote it to.
  it("gives a live connector's own icon no fallback", () => {
    const dataUri = "data:image/svg+xml;base64,AAAA";
    const { detected } = buildPickerWallets(
      [connector(`${curatedKey}:std`, curatedKey, dataUri)],
      false,
    );
    const row = detected.find((w) => w.key === curatedKey);
    expect(row?.icon).toEqual(dataUri);
    expect(row?.iconFallback).toEqual("");
  });

  // An extra wallet has no curated entry to fall back to, so it must not
  // inherit some other wallet's canonical URL.
  it("gives a wallet outside the curated list no fallback", () => {
    const { detected } = buildPickerWallets(
      [connector("brave:std", "Brave", "data:image/svg+xml;base64,BBBB")],
      false,
    );
    const row = detected.find((w) => w.name === "Brave");
    expect(row?.iconFallback).toBeUndefined();
  });
});
