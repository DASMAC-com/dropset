import { useSyncExternalStore } from "react";
import { type ClusterStatus, clusterGate, SUBSCRIPTIONS_RPC_URL } from "../rpc";

// Whether the RPC endpoints serve the cluster the build targets (see
// `createClusterGate`). Every chain read already goes through the gate; this
// is for the UI to show a wrong-network banner and hold the submit CTAs.
// Subscribing is what keeps the gate re-asking while the status is pending.
export function useClusterStatus(): ClusterStatus {
  return useSyncExternalStore(
    clusterGate.subscribe,
    clusterGate.status,
    (): ClusterStatus => "pending",
  );
}

// Whether the subscriptions' websocket is on the build's cluster, as far as
// its provider's RPC endpoint says. Stricter than `useClusterStatus`: a
// mismatched primary leaves the app "verified" on the fallback while its
// websocket still serves the other chain.
export function useSubscriptionsVerified(): boolean {
  return useSyncExternalStore(
    clusterGate.subscribe,
    () => clusterGate.verified(SUBSCRIPTIONS_RPC_URL),
    () => false,
  );
}
