import { useEffect, useSyncExternalStore } from "react";
import { CLUSTER_PROBE_RETRY_MS } from "../data/timings";
import { type ClusterStatus, clusterGate } from "../rpc";

// Whether the RPC endpoints serve the cluster the build targets (see
// `createClusterGate`). Every chain read already goes through the gate; this
// is for the UI to show a wrong-network banner and hold the submit CTAs.
// While no endpoint has answered, it re-asks on its own rather than waiting
// on app traffic, which a panel gated on this status would never send.
export function useClusterStatus(): ClusterStatus {
  const status = useSyncExternalStore(
    clusterGate.subscribe,
    clusterGate.status,
    (): ClusterStatus => "pending",
  );
  useEffect(() => {
    if (status !== "pending") return;
    clusterGate.probe();
    const timer = window.setInterval(clusterGate.probe, CLUSTER_PROBE_RETRY_MS);
    return () => window.clearInterval(timer);
  }, [status]);
  return status;
}
