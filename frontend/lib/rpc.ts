import type { SolanaClientRuntime } from "@solana/client";
import {
  createDefaultRpcTransport,
  createSolanaRpcFromTransport,
  createSolanaRpcSubscriptions,
  type RpcTransport,
} from "@solana/kit";
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
// standing endpoint behind it, else the standing endpoint alone.
// Subscriptions take the primary's websocket when there is one; they do not
// fail over, but the one consumer (the trades tape) already re-subscribes
// after a drop.
export function createAppRuntime(): SolanaClientRuntime {
  const primary = PRIMARY_RPC_URL;
  const fallback = createDefaultRpcTransport({ url: FALLBACK_RPC_URL });
  const transport = primary
    ? createFailoverTransport(
        createDefaultRpcTransport({ url: primary }),
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

// Whether the endpoint really serves the cluster the build targets, asked of
// the chain itself. Memoized per page load on the first caller's rpc (the
// endpoints are the same for every client the page builds); a failed read is
// asked again by the next caller.
let clusterVerified: Promise<boolean> | null = null;
export function verifyCluster(
  rpc: SolanaClientRuntime["rpc"],
): Promise<boolean> {
  clusterVerified ??= rpc
    .getGenesisHash()
    .send()
    .then((seen) => {
      if (genesisMatchesCluster(seen, CLUSTER)) return true;
      console.error(
        `[rpc] genesis ${seen} does not match NEXT_PUBLIC_CLUSTER=${CLUSTER}; refusing to render the order book`,
      );
      return false;
    })
    .catch((error) => {
      // An unreadable genesis is a transport problem, not a wrong chain; let
      // the next caller ask again rather than pinning the panel hidden.
      clusterVerified = null;
      throw error;
    });
  return clusterVerified;
}
