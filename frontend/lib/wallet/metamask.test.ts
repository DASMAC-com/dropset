import type { RpcTransport } from "@solana/kit";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAINNET_GENESIS_HASH } from "../rpc";

// Hoisted alongside the `vi.mock` factories that close over them.
const { createSolanaClient, served } = vi.hoisted(() => ({
  createSolanaClient: vi.fn(async () => ({})),
  served: { genesis: "", fail: false },
}));
vi.mock("@metamask/connect-solana", () => ({ createSolanaClient }));
// A retry interval short enough for the test to wait out several.
vi.mock("../data/timings", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../data/timings")>()),
  CLUSTER_PROBE_RETRY_MS: 2,
}));
// Every transport the module builds answers the genesis read with `served`,
// or fails like a provider that is down while `served.fail` is set.
vi.mock("@solana/kit", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@solana/kit")>()),
  createDefaultRpcTransport: () =>
    (async ({ payload }: Parameters<RpcTransport>[0]) => {
      if (served.fail) throw new Error("HTTP 503");
      return {
        jsonrpc: "2.0",
        id: (payload as { id: string }).id,
        result: served.genesis,
      };
    }) as RpcTransport,
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

  it("runs against a mainnet build", async () => {
    // The refusal below reads a non-mainnet genesis as the wrong cluster.
    expect((await import("../env")).CLUSTER).toBe("mainnet");
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

  it("waits out an unreadable genesis, then registers on the same call", async () => {
    served.fail = true;
    const { registerMetaMaskConnect } = await import("./metamask");
    const registered = registerMetaMaskConnect();
    // Several retry intervals pass with the provider down.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(createSolanaClient).not.toHaveBeenCalled();

    served.fail = false;
    served.genesis = MAINNET_GENESIS_HASH;
    await registered;
    expect(createSolanaClient).toHaveBeenCalledOnce();
  });
});
