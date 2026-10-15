import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { inWalletBrowseUrl, pickerRowAction } from "./mobile";

// Hoisted alongside the `vi.mock` factory that closes over it.
const { registerMwa } = vi.hoisted(() => ({ registerMwa: vi.fn() }));
vi.mock("@solana-mobile/wallet-standard-mobile", () => ({
  createDefaultAuthorizationCache: () => ({}),
  createDefaultChainSelector: () => ({}),
  createDefaultWalletNotFoundHandler: () => () => {},
  registerMwa,
}));

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

const BROWSE = "https://phantom.com/ul/browse/";

describe("pickerRowAction", () => {
  // A live connector wins everywhere: bouncing an installed wallet's user
  // into its in-app browser would replace a working connect with a detour.
  it("connects whenever a connector is registered", () => {
    expect(
      pickerRowAction({ connectorId: "phantom:std", browse: BROWSE }, true),
    ).toEqual("connect");
    expect(
      pickerRowAction({ connectorId: "phantom:std", browse: BROWSE }, false),
    ).toEqual("connect");
  });

  it("opens in the wallet's browser on mobile when there is a link", () => {
    expect(pickerRowAction({ browse: BROWSE }, true)).toEqual("open-in");
  });

  it("falls back to install on desktop or with no link", () => {
    expect(pickerRowAction({ browse: BROWSE }, false)).toEqual("install");
    expect(pickerRowAction({}, true)).toEqual("install");
  });
});

describe("registerMobileWalletAdapter", () => {
  // The latch is module state, so each test takes a fresh module.
  const load = async () =>
    (await import("./mobile")).registerMobileWalletAdapter;

  beforeEach(() => {
    vi.resetModules();
    registerMwa.mockReset();
    vi.stubGlobal("window", { location: { origin: "https://dropset.io" } });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("registers once on Android, however often it is called", async () => {
    vi.stubGlobal("navigator", { userAgent: "Linux; Android 14" });
    const register = await load();
    await Promise.all([register(), register()]);
    await register();
    expect(registerMwa).toHaveBeenCalledTimes(1);
  });

  it("registers nothing off Android", async () => {
    vi.stubGlobal("navigator", { userAgent: "iPhone; CPU iPhone OS 18_0" });
    const register = await load();
    await register();
    expect(registerMwa).not.toHaveBeenCalled();
  });

  // A failed registration must not wedge the latch, or the picker never
  // gains the MWA row until a reload.
  it("clears the latch after a failure so a later call retries", async () => {
    vi.stubGlobal("navigator", { userAgent: "Linux; Android 14" });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    registerMwa.mockImplementationOnce(() => {
      throw new Error("boom");
    });
    const register = await load();
    await register();
    await register();
    expect(registerMwa).toHaveBeenCalledTimes(2);
  });
});
