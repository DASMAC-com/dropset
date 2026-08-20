import { describe, expect, it } from "vitest";
import { resolveWalletIconSrc } from "./WalletIcon";

// The claims WalletIcon's comments make about its fallback. All of them are
// decidable from (primary, fallback, failed) alone, which is why the
// resolution is a pure function rather than inline JSX — the unit runner has
// no DOM and could not otherwise reach any of this.
describe("resolveWalletIconSrc", () => {
  const mirror = "/wallet-icons/phantom.png";
  const canonical = "https://github.com/phantom.png?size=128";

  it("prefers the mirror until it fails", () => {
    expect(resolveWalletIconSrc(mirror, canonical, [])).toBe(mirror);
  });

  it("promotes to the canonical URL once the mirror has failed", () => {
    expect(resolveWalletIconSrc(mirror, canonical, [mirror])).toBe(canonical);
  });

  // The whole point of the issue this component closes: a mirrored path that
  // 404s is truthy, so the picker's old `w.icon ? … : avatar` test could not
  // see it and rendered an empty box. Exhausting both sources has to reach the
  // avatar, not a third dead <img>.
  it("gives up once both sources have failed", () => {
    expect(resolveWalletIconSrc(mirror, canonical, [mirror, canonical])).toBe(
      null,
    );
  });

  // The reason the state holds URLs rather than a count: the picker rebuilds
  // its rows whenever a connector is discovered, swapping icons in place. A
  // count would carry the previous wallet's failures across.
  it("ignores failures recorded against a different wallet", () => {
    const otherMirror = "/wallet-icons/solflare.png";
    expect(resolveWalletIconSrc(mirror, canonical, [otherMirror])).toBe(mirror);
  });

  // A wallet absent from the manifest has the canonical URL as its primary and
  // nothing behind it; a live connector has its own data URI and likewise
  // nothing behind it. Neither may resolve to "".
  it("handles a primary with no fallback behind it", () => {
    expect(resolveWalletIconSrc(canonical, "", [])).toBe(canonical);
    expect(resolveWalletIconSrc(canonical, "", [canonical])).toBe(null);
  });

  it("gives up immediately when there is no icon at all", () => {
    expect(resolveWalletIconSrc("", "", [])).toBe(null);
  });
});
