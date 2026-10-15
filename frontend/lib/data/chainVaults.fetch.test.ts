import { beforeEach, describe, expect, it, vi } from "vitest";

// The route resolver is the only chain read fetchChainVaultMarkets makes, so
// mocking it (and the WASM init) exercises the failure accounting alone.
const resolve = vi.fn();
vi.mock("../eclob/route", () => ({
  resolveEclobRoute: (...args: unknown[]) => resolve(...args),
}));
vi.mock("@dropset/sdk", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@dropset/sdk")>()),
  initSimulator: async () => {},
}));

const { fetchChainVaultMarkets, pairKey } = await import("./chainVaults");
const { stablecoinMint } = await import("./currencies");

const rpc = {} as Parameters<typeof fetchChainVaultMarkets>[0];

describe("fetchChainVaultMarkets", () => {
  beforeEach(() => resolve.mockReset());

  it("reads a cluster with no markets as empty, not as an outage", async () => {
    resolve.mockResolvedValue(null);
    await expect(fetchChainVaultMarkets(rpc)).resolves.toEqual({
      markets: [],
      failed: new Set(),
    });
  });

  it("throws when the only failures leave no market at all", async () => {
    // The tGBP market errors; every other pair has no market. That is an
    // outage, and must not read as "no vaults on-chain yet".
    const tgbp = stablecoinMint("TGBP");
    resolve.mockImplementation(async (_rpc: unknown, from: string) =>
      from === tgbp ? Promise.reject(new Error("decode")) : null,
    );
    await expect(fetchChainVaultMarkets(rpc)).rejects.toThrow(
      "no vault market could be read",
    );
  });
});

describe("pairKey", () => {
  it("ignores orientation", () => {
    expect(pairKey("TGBP", "USDC")).toBe(pairKey("USDC", "TGBP"));
  });
});
