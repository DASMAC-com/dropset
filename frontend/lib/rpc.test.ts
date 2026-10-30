import type { RpcTransport } from "@solana/kit";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  assertEndpointMatchesCluster,
  ClusterMismatchError,
  createClusterGate,
  createFailoverTransport,
  endpointHost,
  genesisMatchesCluster,
  MAINNET_GENESIS_HASH,
} from "./rpc";

const ok = (label: string) =>
  vi.fn(async () => ({ result: label })) as unknown as RpcTransport &
    ReturnType<typeof vi.fn>;
const failing = () =>
  vi.fn(async () => {
    throw new Error("HTTP 429");
  }) as unknown as RpcTransport & ReturnType<typeof vi.fn>;
const call = (transport: RpcTransport, signal?: AbortSignal) =>
  transport({ payload: { method: "getSlot" }, signal });

describe("failover transport", () => {
  it("serves from the primary while it answers", async () => {
    const primary = ok("primary");
    const fallback = ok("fallback");
    const t = createFailoverTransport(primary, fallback);
    await expect(call(t)).resolves.toEqual({ result: "primary" });
    expect(fallback).not.toHaveBeenCalled();
  });

  it("retries a failed call on the fallback, then skips the primary for the cooldown", async () => {
    let clock = 0;
    const primary = failing();
    const fallback = ok("fallback");
    const onFailover = vi.fn();
    const t = createFailoverTransport(primary, fallback, {
      cooldownMs: 1000,
      now: () => clock,
      onFailover,
    });

    await expect(call(t)).resolves.toEqual({ result: "fallback" });
    expect(onFailover).toHaveBeenCalledOnce();

    clock = 999;
    await call(t);
    expect(primary).toHaveBeenCalledOnce();

    clock = 1000;
    await call(t);
    expect(primary).toHaveBeenCalledTimes(2);
  });

  it("does not retry a call the caller aborted", async () => {
    const abort = new AbortController();
    const primary = vi.fn(async () => {
      abort.abort();
      throw new Error("aborted");
    }) as unknown as RpcTransport;
    const fallback = ok("fallback");
    const t = createFailoverTransport(primary, fallback);
    await expect(call(t, abort.signal)).rejects.toThrow("aborted");
    expect(fallback).not.toHaveBeenCalled();
  });
});

describe("endpoint checks", () => {
  it("refuses a mainnet build on a loopback endpoint", () => {
    for (const url of [
      "http://127.0.0.1:8899",
      "http://localhost:8899",
      "http://[::1]:8899",
      "http://0.0.0.0:8899",
    ]) {
      expect(() =>
        assertEndpointMatchesCluster("NEXT_PUBLIC_RPC_URL", url, "mainnet"),
      ).toThrow(/NEXT_PUBLIC_RPC_URL/);
    }
  });

  it("accepts a remote mainnet endpoint and any localnet endpoint", () => {
    expect(() =>
      assertEndpointMatchesCluster(
        "NEXT_PUBLIC_RPC_URL",
        "https://solana-rpc.publicnode.com",
        "mainnet",
      ),
    ).not.toThrow();
    expect(() =>
      assertEndpointMatchesCluster(
        "NEXT_PUBLIC_RPC_URL",
        "http://192.168.1.20:8899",
        "localnet",
      ),
    ).not.toThrow();
  });

  it("names a provider by host, never by its keyed URL", () => {
    expect(
      endpointHost("https://mainnet.helius-rpc.com/?api-key=secret-key"),
    ).toBe("mainnet.helius-rpc.com");
  });

  it("matches genesis to the cluster", () => {
    expect(genesisMatchesCluster(MAINNET_GENESIS_HASH, "mainnet")).toBe(true);
    expect(genesisMatchesCluster("FreshLocalGenesis1111", "mainnet")).toBe(
      false,
    );
    expect(genesisMatchesCluster("FreshLocalGenesis1111", "localnet")).toBe(
      true,
    );
    expect(genesisMatchesCluster(MAINNET_GENESIS_HASH, "localnet")).toBe(false);
  });
});

describe("cluster gate", () => {
  const PRIMARY = "https://primary.test";
  const FALLBACK = "https://fallback.test";
  // An endpoint serving `genesis`, answering any other call with `label`.
  const chain = (genesis: string, label: string) =>
    vi.fn(async ({ payload }: Parameters<RpcTransport>[0]) => {
      const { id, method } = payload as { id: string; method: string };
      return method === "getGenesisHash"
        ? { jsonrpc: "2.0", id, result: genesis }
        : { result: label };
    }) as unknown as RpcTransport & ReturnType<typeof vi.fn>;
  const genesisReads = (t: ReturnType<typeof vi.fn>) =>
    t.mock.calls.filter(
      ([config]) =>
        (config as { payload: { method: string } }).payload.method ===
        "getGenesisHash",
    ).length;
  const quiet = () => vi.spyOn(console, "error").mockImplementation(() => {});

  it("checks each endpoint once, then serves through it", async () => {
    const gate = createClusterGate("mainnet");
    const raw = chain(MAINNET_GENESIS_HASH, "ok");
    const t = gate.guard(PRIMARY, raw);
    expect(gate.status()).toBe("pending");
    await expect(call(t)).resolves.toEqual({ result: "ok" });
    await call(t);
    expect(genesisReads(raw)).toBe(1);
    expect(gate.status()).toBe("verified");
  });

  it("refuses a primary on another chain and fails over to the fallback", async () => {
    const error = quiet();
    const gate = createClusterGate("mainnet");
    const t = createFailoverTransport(
      gate.guard(PRIMARY, chain("DevnetGenesis1111", "devnet")),
      gate.guard(FALLBACK, chain(MAINNET_GENESIS_HASH, "mainnet")),
    );
    await expect(call(t)).resolves.toEqual({ result: "mainnet" });
    expect(gate.status()).toBe("verified");
    error.mockRestore();
  });

  it("refuses a fallback on another chain rather than failing over to it", async () => {
    const error = quiet();
    const gate = createClusterGate("mainnet");
    const fallback = chain("DevnetGenesis1111", "devnet");
    const t = createFailoverTransport(
      failing(),
      gate.guard(FALLBACK, fallback),
    );
    await expect(call(t)).rejects.toBeInstanceOf(ClusterMismatchError);
    expect(fallback).toHaveBeenCalledOnce();
    error.mockRestore();
  });

  it("reads mismatch only once every endpoint has answered another chain", async () => {
    const error = quiet();
    const gate = createClusterGate("mainnet");
    const listener = vi.fn();
    gate.subscribe(listener);
    const primary = gate.guard(PRIMARY, chain("DevnetGenesis1111", "a"));
    const fallback = gate.guard(FALLBACK, chain("DevnetGenesis1111", "b"));
    await expect(call(primary)).rejects.toBeInstanceOf(ClusterMismatchError);
    expect(gate.status()).toBe("pending");
    await expect(call(fallback)).rejects.toBeInstanceOf(ClusterMismatchError);
    expect(gate.status()).toBe("mismatch");
    expect(listener).toHaveBeenCalledTimes(2);
    expect(error).toHaveBeenCalledTimes(2);
    error.mockRestore();
  });

  it("asks again after an unreadable genesis", async () => {
    const gate = createClusterGate("mainnet");
    let up = false;
    const raw = vi.fn(async (config: Parameters<RpcTransport>[0]) => {
      if (!up) throw new Error("HTTP 503");
      return chain(MAINNET_GENESIS_HASH, "ok")(config);
    }) as unknown as RpcTransport;
    const t = gate.guard(PRIMARY, raw);
    await expect(call(t)).rejects.toThrow("HTTP 503");
    expect(gate.status()).toBe("pending");
    up = true;
    await expect(call(t)).resolves.toEqual({ result: "ok" });
    expect(gate.status()).toBe("verified");
  });

  it("probes every guarded endpoint without app traffic", async () => {
    const gate = createClusterGate("localnet");
    const raw = chain("FreshLocalGenesis1111", "ok");
    gate.guard(PRIMARY, raw);
    gate.probe();
    await vi.waitFor(() => expect(gate.status()).toBe("verified"));
    expect(genesisReads(raw)).toBe(1);
    expect(gate.verified(PRIMARY)).toBe(true);
  });

  it("shares one genesis read between concurrent first calls", async () => {
    const gate = createClusterGate("mainnet");
    const raw = chain(MAINNET_GENESIS_HASH, "ok");
    const t = gate.guard(PRIMARY, raw);
    await Promise.all([call(t), call(t), call(t)]);
    expect(genesisReads(raw)).toBe(1);
  });

  it("gives up on a genesis read that never answers, then asks again", async () => {
    const gate = createClusterGate("mainnet", { timeoutMs: 20 });
    let hang = true;
    const raw = vi.fn(async (config: Parameters<RpcTransport>[0]) =>
      hang ? new Promise(() => {}) : chain(MAINNET_GENESIS_HASH, "ok")(config),
    ) as unknown as RpcTransport;
    const t = gate.guard(PRIMARY, raw);
    await expect(call(t)).rejects.toThrow();
    hang = false;
    await expect(call(t)).resolves.toEqual({ result: "ok" });
  });

  it("lets a caller abort a call still waiting on the genesis read", async () => {
    const gate = createClusterGate("mainnet");
    const raw = vi.fn(() => new Promise(() => {})) as unknown as RpcTransport;
    const abort = new AbortController();
    const pending = call(gate.guard(PRIMARY, raw), abort.signal);
    abort.abort(new Error("caller gave up"));
    await expect(pending).rejects.toThrow("caller gave up");
  });

  it("re-asks on its own while subscribed and pending, and stops after", async () => {
    vi.useFakeTimers();
    const gate = createClusterGate("mainnet", { retryMs: 1000 });
    let up = false;
    const raw = vi.fn(async (config: Parameters<RpcTransport>[0]) => {
      if (!up) throw new Error("HTTP 503");
      return chain(MAINNET_GENESIS_HASH, "ok")(config);
    }) as unknown as RpcTransport & ReturnType<typeof vi.fn>;
    gate.guard(PRIMARY, raw);
    const unsubscribe = gate.subscribe(() => {});
    await vi.advanceTimersByTimeAsync(0);
    expect(raw).toHaveBeenCalledTimes(1);
    up = true;
    await vi.advanceTimersByTimeAsync(1000);
    expect(gate.status()).toBe("verified");
    await vi.advanceTimersByTimeAsync(5000);
    expect(raw).toHaveBeenCalledTimes(2);
    unsubscribe();
    vi.useRealTimers();
  });
});

// The wiring the gate's claim rests on: both sides of the app runtime are
// guarded, so neither can serve a read from another chain.
describe("app runtime", () => {
  const runtimeWith = async (genesis: Record<string, string>) => {
    vi.resetModules();
    vi.stubEnv("NEXT_PUBLIC_RPC_URL", "https://fallback.test");
    vi.stubEnv("NEXT_PUBLIC_WS_URL", "wss://fallback.test");
    vi.stubEnv("NEXT_PUBLIC_PRIMARY_RPC_URL", "https://primary.test");
    vi.stubEnv("NEXT_PUBLIC_PRIMARY_WS_URL", "wss://primary.test");
    vi.doMock("@solana/kit", async (original) => ({
      ...(await original<typeof import("@solana/kit")>()),
      createDefaultRpcTransport: ({ url }: { url: string }) =>
        (async ({ payload }: { payload: { id: string; method: string } }) =>
          payload.method === "getGenesisHash"
            ? { jsonrpc: "2.0", id: payload.id, result: genesis[url] }
            : {
                jsonrpc: "2.0",
                id: payload.id,
                result: { context: { slot: 1n }, value: url },
              }) as unknown as RpcTransport,
    }));
    const { createAppRuntime } = await import("./rpc");
    return createAppRuntime();
  };
  const served = async (genesis: Record<string, string>) =>
    (
      (await (
        await runtimeWith(genesis)
      ).rpc
        .getBalance("11111111111111111111111111111111" as never)
        .send()) as unknown as { value: string }
    ).value;

  afterEach(() => {
    vi.doUnmock("@solana/kit");
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("serves from the fallback when the primary is on another chain", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(
      served({
        "https://primary.test": "DevnetGenesis1111",
        "https://fallback.test": MAINNET_GENESIS_HASH,
      }),
    ).resolves.toBe("https://fallback.test");
    error.mockRestore();
    warn.mockRestore();
  });

  it("refuses when both sides are on another chain", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(
      served({
        "https://primary.test": "DevnetGenesis1111",
        "https://fallback.test": "DevnetGenesis1111",
      }),
    ).rejects.toThrow(/does not serve/);
    error.mockRestore();
    warn.mockRestore();
  });
});
