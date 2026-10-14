import type { RpcTransport } from "@solana/kit";
import { describe, expect, it, vi } from "vitest";

import {
  assertEndpointMatchesCluster,
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

// The test env leaves NEXT_PUBLIC_CLUSTER unset, so these run as mainnet.
// Each test loads a fresh module so the page-load memo starts empty.
describe("verifyCluster", () => {
  const fakeRpc = (send: () => Promise<string>) =>
    ({ getGenesisHash: () => ({ send }) }) as unknown as Parameters<
      typeof import("./rpc").verifyCluster
    >[0];
  const fresh = async () => {
    vi.resetModules();
    return (await import("./rpc")).verifyCluster;
  };

  it("reads the genesis once per page load", async () => {
    const verifyCluster = await fresh();
    const send = vi.fn(async () => MAINNET_GENESIS_HASH);
    await expect(verifyCluster(fakeRpc(send))).resolves.toBe(true);
    await expect(verifyCluster(fakeRpc(send))).resolves.toBe(true);
    expect(send).toHaveBeenCalledOnce();
  });

  it("refuses a chain that is not the build's cluster", async () => {
    const verifyCluster = await fresh();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    await expect(
      verifyCluster(fakeRpc(async () => "FreshLocalGenesis1111")),
    ).resolves.toBe(false);
    expect(error).toHaveBeenCalledOnce();
    error.mockRestore();
  });

  it("asks again after a failed read", async () => {
    const verifyCluster = await fresh();
    const failing = fakeRpc(async () => {
      throw new Error("HTTP 503");
    });
    await expect(verifyCluster(failing)).rejects.toThrow("HTTP 503");
    const send = vi.fn(async () => MAINNET_GENESIS_HASH);
    await expect(verifyCluster(fakeRpc(send))).resolves.toBe(true);
    expect(send).toHaveBeenCalledOnce();
  });
});
