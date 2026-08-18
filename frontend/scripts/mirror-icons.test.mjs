import { describe, expect, it } from "vitest";
import { sniffExt } from "./mirror-icons.mjs";

// The format check is the whole reason this module exists: an issuer CDN can
// answer 200 with an HTML error page, and deriving the extension from the
// content-type header wrote that page to disk as an icon and recorded it in
// the manifest as a success — a broken icon indistinguishable from a working
// one. These pin the byte-level decision that replaced the header.
describe("sniffExt", () => {
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const jpg = Buffer.from([0xff, 0xd8, 0xff, 0xe0]);
  const gif = Buffer.from("GIF89a");
  const webp = Buffer.concat([
    Buffer.from("RIFF"),
    Buffer.from([0, 0, 0, 0]),
    Buffer.from("WEBP"),
  ]);

  it("identifies the binary formats from their magic bytes", () => {
    expect(sniffExt(png)).toBe("png");
    expect(sniffExt(jpg)).toBe("jpg");
    expect(sniffExt(gif)).toBe("gif");
    expect(sniffExt(webp)).toBe("webp");
  });

  it("accepts an SVG that opens the document, behind a prolog or not", () => {
    expect(
      sniffExt(Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>')),
    ).toBe("svg");
    // The minimal root: no attributes, self-closing, so the character after
    // the tag name is `/` rather than whitespace or `>`.
    expect(sniffExt(Buffer.from("<svg/>"))).toBe("svg");
    expect(
      sniffExt(Buffer.from('<?xml version="1.0"?>\n<svg viewBox="0 0 1 1"/>')),
    ).toBe("svg");
    expect(sniffExt(Buffer.from("\n  <!-- a note -->\n<svg/>"))).toBe("svg");
  });

  // The regression this module exists to prevent. A CDN challenge or error
  // page routinely embeds an inline logo, so testing for `<svg` anywhere in
  // the body — rather than at the start of the document — accepts the very
  // interstitial the sniffing was added to reject.
  it("rejects an HTML page that merely contains an inline <svg>", () => {
    const interstitial = Buffer.from(
      "<!doctype html><html><head><title>Access denied</title></head>" +
        '<body><svg viewBox="0 0 24 24"><path d="M0 0h24v24H0z"/></svg>' +
        "<p>Attention Required</p></body></html>",
    );
    expect(sniffExt(interstitial)).toBeUndefined();
  });

  it("rejects anything else", () => {
    expect(sniffExt(Buffer.from("not an image at all"))).toBeUndefined();
    expect(sniffExt(Buffer.from(""))).toBeUndefined();
    expect(sniffExt(Buffer.from('{"error":"not found"}'))).toBeUndefined();
  });
});
