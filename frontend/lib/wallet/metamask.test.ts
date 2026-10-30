import type { RpcTransport } from "@solana/kit";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAINNET_GENESIS_HASH } from "../rpc";

// Hoisted alongside the `vi.mock` factories that close over them.
const { createSolanaClient, served } = vi.hoisted(() => ({
  createSolanaClient: vi.fn(async () => ({})),
  served: { genesis: "" },
}));
vi.mock("@metamask/connect-solana", () => ({ createSolanaClient }));
// Every transport the module builds answers the genesis read with `served`.
vi.mock("@solana/kit", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@solana/kit")>()),
  createDefaultRpcTransport: () =>
    (async ({ payload }: Parameters<RpcTransport>[0]) => ({
      jsonrpc: "2.0",
      id: (payload as { id: string }).id,
      result: served.genesis,
    })) as RpcTransport,
}));

describe("MetaMask Connect registration", () => {
  beforeEach(() => {
    // A fresh module per test: the registration latch and the page's gate
    // verdicts are both module state.
    vi.resetModules();
    createSolanaClient.mockClear();
    vi.stubGlobal("window", { location: { origin: "https://dropset.test" } });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("registers once the endpoint serves the build's cluster", async () => {
    served.genesis = MAINNET_GENESIS_HASH;
    const { registerMetaMaskConnect } = await import("./metamask");
    await registerMetaMaskConnect();
    expect(createSolanaClient).toHaveBeenCalledOnce();
  });

  it("refuses a wrong-cluster endpoint through the shared gate", async () => {
    served.genesis = "DevnetGenesis1111";
    vi.spyOn(console, "error").mockImplementation(() => {});
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { registerMetaMaskConnect } = await import("./metamask");
    const { clusterGate } = await import("../rpc");
    await registerMetaMaskConnect();
    expect(createSolanaClient).not.toHaveBeenCalled();
    expect(clusterGate.status()).toBe("mismatch");
    expect(warn.mock.calls[0]?.[1]).toMatchObject({
      name: "ClusterMismatchError",
    });
  });
});
