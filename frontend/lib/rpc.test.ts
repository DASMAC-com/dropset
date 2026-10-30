import type { RpcTransport } from "@solana/kit";
import { describe, expect, it, vi } from "vitest";

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
  });
});
