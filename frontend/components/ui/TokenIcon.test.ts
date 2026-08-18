import { describe, expect, it } from "vitest";
import { resolveIconSrc } from "./TokenIcon";

// The three claims TokenIcon's comments make about its fallback. All of them
// are decidable from (primary, fallback, failedSrc) alone, which is why the
// resolution is a pure function rather than inline JSX — the unit runner has
// no DOM and could not otherwise reach any of this.
describe("resolveIconSrc", () => {
  const mirror = "/token-icons/EURC.png";
  const canonical = "https://issuer.example/eurc.png";

  it("prefers the mirror until it fails", () => {
    expect(resolveIconSrc(mirror, canonical, null)).toBe(mirror);
  });

  it("promotes to the canonical URL once the mirror has failed", () => {
    expect(resolveIconSrc(mirror, canonical, mirror)).toBe(canonical);
  });

  // Recording a fallback failure as well would flip the source back to the
  // mirror on the next render, and the two dead URLs would alternate forever.
  it("does not flip back when the fallback itself is what failed", () => {
    expect(resolveIconSrc(mirror, canonical, canonical)).toBe(mirror);
  });

  // The reason the state holds a URL rather than a boolean: these components
  // render in reused list rows and in the picker trigger, which swap symbols
  // in place. A boolean would carry the previous symbol's failure across.
  it("ignores a failure recorded against a different symbol", () => {
    const otherMirror = "/token-icons/USDC.png";
    expect(resolveIconSrc(mirror, canonical, otherMirror)).toBe(mirror);
  });

  // A symbol absent from the manifest has the canonical URL as its primary
  // and nothing behind it, so a failure must not resolve to "".
  it("stays on the primary when there is no fallback to promote", () => {
    expect(resolveIconSrc(canonical, "", canonical)).toBe(canonical);
    expect(resolveIconSrc("", "", null)).toBe("");
  });
});
