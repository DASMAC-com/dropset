// cspell:word KHTML

import { afterEach, describe, expect, it, vi } from "vitest";
import { isMobile } from "./ua";

const IPHONE_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
// What iPadOS Safari sends by default: the desktop Mac UA, byte for byte.
const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const ANDROID_UA =
  "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Mobile Safari/537.36";

const stubNavigator = (userAgent: string, maxTouchPoints: number) =>
  vi.stubGlobal("navigator", { userAgent, maxTouchPoints });

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("isMobile", () => {
  it("is true for an iPhone and an Android phone", () => {
    stubNavigator(IPHONE_UA, 5);
    expect(isMobile()).toBe(true);
    stubNavigator(ANDROID_UA, 5);
    expect(isMobile()).toBe(true);
  });

  // Without this an iPad gets the desktop picker: no "Open in" rows, and a
  // handoff QR it cannot scan with its own camera.
  it("is true for an iPad reporting a Mac UA", () => {
    stubNavigator(MAC_UA, 5);
    expect(isMobile()).toBe(true);
  });

  it("is false for a Mac, which has no touch screen", () => {
    stubNavigator(MAC_UA, 0);
    expect(isMobile()).toBe(false);
  });
});
