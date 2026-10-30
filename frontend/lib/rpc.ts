import type { SolanaClientRuntime } from "@solana/client";
import {
  createDefaultRpcTransport,
  createSolanaRpcFromTransport,
  createSolanaRpcSubscriptions,
  type RpcTransport,
} from "@solana/kit";
import {
  CLUSTER_CHECK_TIMEOUT_MS,
  CLUSTER_PROBE_RETRY_MS,
} from "./data/timings";
import {
  CLUSTER,
  type Cluster,
  FALLBACK_RPC_URL,
  FALLBACK_WS_URL,
  PRIMARY_RPC_URL,
  PRIMARY_WS_URL,
} from "./env";

// How long every call skips the primary after it fails. A primary that is
// down (or rate-limiting) would otherwise cost each call a failed round-trip
// before its fallback; a primary that recovered is back within one window.
export const PRIMARY_COOLDOWN_MS = 30_000;

// Solana mainnet-beta's genesis hash, the chain's permanent identity. Mirrors
// the TUI's `MAINNET_GENESIS_HASH` (tui/src/cluster.rs).
export const MAINNET_GENESIS_HASH =
  "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";

const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "0.0.0.0", "[::1]"]);

// Hostname of an endpoint, for the provider note: never the full URL, which
// carries the API key in its path or query.
export const endpointHost = (url: string): string => new URL(url).hostname;

// Refuse a mainnet build pointed at a local validator: it would route to
// DFlow against a chain it cannot see. Throws, naming the variable to fix.
// The converse is left unchecked: a localnet build may legitimately reach
// its validator over a LAN address (a phone on the same network).
export function assertEndpointMatchesCluster(
  name: string,
  url: string,
  cluster: Cluster,
): void {
  if (cluster === "mainnet" && LOOPBACK_HOSTS.has(new URL(url).hostname)) {
    throw new Error(
      `${name} points at a local validator (${endpointHost(url)}) but NEXT_PUBLIC_CLUSTER is mainnet. Set NEXT_PUBLIC_CLUSTER=localnet, or point it at a mainnet provider.`,
    );
  }
}

// A transport that sends to `primary` and retries a failed call on
// `fallback`. A transport throws only when the provider fails (network error,
// any non-2xx: a 429, or a 403 from the key's origin restriction); a JSON-RPC
// error comes back as a normal response, so a rejected transaction is never
// replayed on the second provider. A caller's abort is respected, never
// retried.
export function createFailoverTransport(
  primary: RpcTransport,
  fallback: RpcTransport,
  {
    cooldownMs = PRIMARY_COOLDOWN_MS,
    now = Date.now,
    onFailover,
  }: {
    cooldownMs?: number;
    now?: () => number;
    onFailover?: (error: unknown) => void;
  } = {},
): RpcTransport {
  let skipPrimaryUntil = 0;
  return (async (config: Parameters<RpcTransport>[0]) => {
    if (now() >= skipPrimaryUntil) {
      try {
        return await primary(config);
      } catch (error) {
        if (config.signal?.aborted) throw error;
        skipPrimaryUntil = now() + cooldownMs;
        onFailover?.(error);
      }
    }
    return fallback(config);
  }) as RpcTransport;
}

// The app's RPC runtime: the keyed primary when one is configured, with the
// standing endpoint behind it, else the standing endpoint alone. Each side is
// behind the cluster gate: a primary serving another chain is refused and the
// call fails over to the fallback; a fallback serving another chain is
// refused outright.
// Subscriptions take the primary's websocket when there is one; they do not
// fail over, but the one consumer (the trades tape) already re-subscribes
// after a drop. The websocket cannot be asked its genesis, so the tape waits
// on the verdict of the RPC endpoint from the same provider
// (`SUBSCRIPTIONS_RPC_URL`).
export function createAppRuntime(): SolanaClientRuntime {
  const primary = PRIMARY_RPC_URL;
  const fallback = clusterGate.guard(
    FALLBACK_RPC_URL,
    createDefaultRpcTransport({ url: FALLBACK_RPC_URL }),
  );
  const transport = primary
    ? createFailoverTransport(
        clusterGate.guard(primary, createDefaultRpcTransport({ url: primary })),
        fallback,
        {
          onFailover: (error) =>
            console.warn(
              `[rpc] ${endpointHost(primary)} failed; using ${endpointHost(FALLBACK_RPC_URL)} for ${PRIMARY_COOLDOWN_MS / 1000}s`,
              error,
            ),
        },
      )
    : fallback;
  return {
    rpc: createSolanaRpcFromTransport(transport) as SolanaClientRuntime["rpc"],
    rpcSubscriptions: createSolanaRpcSubscriptions(
      PRIMARY_WS_URL ?? FALLBACK_WS_URL,
    ),
  };
}

// The RPC endpoint whose provider serves the subscriptions' websocket (env.ts
// requires the primary's RPC and WS URLs together).
export const SUBSCRIPTIONS_RPC_URL = PRIMARY_RPC_URL ?? FALLBACK_RPC_URL;

// The one-line note naming the live provider, so a post-event deploy still
// pointed at the paid endpoint is visible in any browser console.
export function providerNote(): string {
  const fallback = endpointHost(FALLBACK_RPC_URL);
  return PRIMARY_RPC_URL
    ? `[rpc] ${CLUSTER}: primary ${endpointHost(PRIMARY_RPC_URL)}, fallback ${fallback}`
    : `[rpc] ${CLUSTER}: ${fallback} (no primary configured)`;
}

// Whether a genesis hash is the chain the build targets. A test validator
// mints a fresh genesis on every spawn, so localnet has no hash to match;
// it only has to not be mainnet (the TUI's `Identity::of_local` rule).
export const genesisMatchesCluster = (seen: string, cluster: Cluster) =>
  cluster === "mainnet"
    ? seen === MAINNET_GENESIS_HASH
    : seen !== MAINNET_GENESIS_HASH;

// Thrown in place of any call to an endpoint whose chain is not the build's
// cluster. The failover transport treats it like any provider failure, so a
// mismatched primary is skipped for the fallback.
export class ClusterMismatchError extends Error {
  constructor(url: string, cluster: Cluster) {
    super(
      `${endpointHost(url)} does not serve NEXT_PUBLIC_CLUSTER=${cluster}; refusing to use it`,
    );
    this.name = "ClusterMismatchError";
  }
}

// "verified" once some endpoint serves the build's cluster; "mismatch" once
// every endpoint has answered with another chain; "pending" until either.
export type ClusterStatus = "pending" | "verified" | "mismatch";

export type ClusterGate = {
  // Wrap one endpoint's transport so every call first checks that endpoint's
  // genesis (once per page load) and is refused on another chain.
  guard: (url: string, transport: RpcTransport) => RpcTransport;
  // Check one endpoint's genesis up front, for a consumer that passes the
  // bare URL on rather than calling through a guarded transport (MetaMask
  // Connect). Registers the endpoint like `guard`, so `probe` and `status`
  // count it; resolves whether it serves the build's cluster, and rejects on
  // an unreadable genesis.
  verify: (url: string, transport: RpcTransport) => Promise<boolean>;
  // Check every guarded endpoint now, without waiting for app traffic.
  probe: () => void;
  status: () => ClusterStatus;
  // Whether this one endpoint has answered with the build's cluster.
  verified: (url: string) => boolean;
  // While anyone is subscribed and the status is pending, the gate probes on
  // its own every `retryMs`: a panel gated on the status sends no traffic
  // that would ask again.
  subscribe: (listener: () => void) => () => void;
};

// A promise that also rejects once `signal` aborts, so a caller's abort frees
// it from a genesis check it shares with other calls.
const untilAborted = <T>(promise: Promise<T>, signal?: AbortSignal) =>
  signal
    ? new Promise<T>((resolve, reject) => {
        const onAbort = () => reject(signal.reason);
        if (signal.aborted) {
          // The shared promise still settles later; nobody else may await it.
          promise.catch(() => {});
          return onAbort();
        }
        signal.addEventListener("abort", onAbort, { once: true });
        promise
          .then(resolve, reject)
          .finally(() => signal.removeEventListener("abort", onAbort));
      })
    : promise;

// Each endpoint is checked on its own: a failover pair whose two sides serve
// different chains would otherwise pass on whichever side answered first.
// Verdicts are keyed by URL, so the clients a page rebuilds share them. An
// unreadable genesis (an error, or no answer within `timeoutMs`) is a
// transport problem, not a wrong chain: the next call asks again rather than
// pinning the endpoint refused.
export function createClusterGate(
  cluster: Cluster,
  {
    timeoutMs = CLUSTER_CHECK_TIMEOUT_MS,
    retryMs = CLUSTER_PROBE_RETRY_MS,
  }: { timeoutMs?: number; retryMs?: number } = {},
): ClusterGate {
  const transports = new Map<string, RpcTransport>();
  const checks = new Map<string, Promise<boolean>>();
  const verdicts = new Map<string, boolean>();
  const listeners = new Set<() => void>();
  let retry: ReturnType<typeof setInterval> | undefined;

  const check = (url: string, transport: RpcTransport): Promise<boolean> => {
    let pending = checks.get(url);
    if (!pending) {
      const timeout = AbortSignal.timeout(timeoutMs);
      pending = untilAborted(
        createSolanaRpcFromTransport(transport)
          .getGenesisHash()
          .send({ abortSignal: timeout }),
        timeout,
      ).then(
        (seen) => {
          const ok = genesisMatchesCluster(seen, cluster);
          if (!ok)
            console.error(
              `[rpc] ${endpointHost(url)} serves genesis ${seen}, not NEXT_PUBLIC_CLUSTER=${cluster}; refusing it`,
            );
          verdicts.set(url, ok);
          if (status() !== "pending") stopRetry();
          for (const listener of listeners) listener();
          return ok;
        },
        (error) => {
          checks.delete(url);
          throw error;
        },
      );
      checks.set(url, pending);
    }
    return pending;
  };

  const status = (): ClusterStatus => {
    const seen = [...verdicts.values()];
    if (seen.includes(true)) return "verified";
    return seen.length > 0 && seen.length === transports.size
      ? "mismatch"
      : "pending";
  };

  const probe = () => {
    for (const [url, transport] of transports)
      check(url, transport).catch(() => {});
  };
  const stopRetry = () => {
    clearInterval(retry);
    retry = undefined;
  };

  return {
    guard: (url, transport) => {
      transports.set(url, transport);
      return (async (config: Parameters<RpcTransport>[0]) => {
        if (!(await untilAborted(check(url, transport), config.signal)))
          throw new ClusterMismatchError(url, cluster);
        return transport(config);
      }) as RpcTransport;
    },
    verify: (url, transport) => {
      transports.set(url, transport);
      return check(url, transport);
    },
    probe,
    status,
    verified: (url) => verdicts.get(url) === true,
    subscribe: (listener) => {
      listeners.add(listener);
      if (status() === "pending" && retry === undefined) {
        probe();
        retry = setInterval(probe, retryMs);
      }
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0) stopRetry();
      };
    },
  };
}

// The page's one gate, shared by every client `createAppRuntime` builds.
export const clusterGate = createClusterGate(CLUSTER);
