import { describe, expect, it } from "vitest";
import { inWalletBrowseUrl } from "./mobile";

// Pins the documented `<prefix><url>?ref=<ref>` shape the three in-app browser
// links share. Both values must be encoded: an unencoded page URL carries its
// own `?` and `&`, which the wallet would read as its own query parameters.
describe("inWalletBrowseUrl", () => {
  it("encodes the page URL and the ref", () => {
    expect(
      inWalletBrowseUrl(
        "https://phantom.com/ul/browse/",
        "https://dropset.io/swap?from=USDC&to=tGBP",
        "https://dropset.io",
      ),
    ).toEqual(
      "https://phantom.com/ul/browse/https%3A%2F%2Fdropset.io%2Fswap%3Ffrom%3DUSDC%26to%3DtGBP?ref=https%3A%2F%2Fdropset.io",
    );
  });

  it("returns undefined for a wallet with no published link", () => {
    expect(
      inWalletBrowseUrl(undefined, "https://dropset.io", "https://dropset.io"),
    ).toBeUndefined();
  });
});
