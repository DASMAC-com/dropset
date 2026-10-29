// Server-only: a per-client token bucket for the keyed proxy routes (see
// `forward` in lib/server/apiProxy.ts). The key's quota upstream is one pool
// shared by every user, so a script calling a route in a loop would starve
// everyone else; this caps each caller's share before the key is spent.
//
// The buckets live in the server instance's memory, so the bound is per
// instance: a deploy running several instances admits up to that many times
// the limit from one caller. That still stops one client from draining the
// pool in a burst, which is the failure being guarded against; a platform
// firewall rule on /api/dflow/* is the stronger, deploy-wide complement.

export type ClientLimit = {
  // Requests a caller may make back to back from a full bucket.
  capacity: number;
  // Tokens restored per second, up to `capacity`.
  refillPerSec: number;
};

type Bucket = { tokens: number; updatedAt: number };

// Past this many tracked callers, buckets that have refilled to capacity
// (indistinguishable from an untracked caller) are dropped, and the oldest
// are evicted if that is not enough, so a flood of spoofed or rotating
// addresses cannot grow the map without bound.
export const MAX_CLIENTS = 10_000;
const EVICT_TO = MAX_CLIENTS * 0.9;

export type ClientLimiter = {
  // Spend one of the caller's tokens: null when the request may proceed, or
  // the whole seconds until a token is back when it may not.
  take: (client: string, now?: number) => number | null;
  // Callers currently tracked.
  size: () => number;
};

export function clientLimiter(limit: ClientLimit): ClientLimiter {
  const buckets = new Map<string, Bucket>();
  const refilled = (b: Bucket, now: number): number =>
    Math.min(
      limit.capacity,
      b.tokens + (Math.max(0, now - b.updatedAt) / 1000) * limit.refillPerSec,
    );

  const prune = (now: number) => {
    for (const [client, b] of buckets) {
      if (refilled(b, now) >= limit.capacity) buckets.delete(client);
    }
    // Map iterates in insertion order, and a caller is re-inserted on every
    // request, so the first entries are the least recently seen. Evicting to
    // below the cap, not just under it, keeps a flood from paying this scan
    // on every request.
    for (const client of buckets.keys()) {
      if (buckets.size <= EVICT_TO) break;
      buckets.delete(client);
    }
  };

  return {
    take(client, now = Date.now()) {
      const prev = buckets.get(client);
      const tokens = prev ? refilled(prev, now) : limit.capacity;
      if (tokens < 1) {
        return Math.max(1, Math.ceil((1 - tokens) / limit.refillPerSec));
      }
      buckets.delete(client);
      if (buckets.size >= MAX_CLIENTS) prune(now);
      buckets.set(client, { tokens: tokens - 1, updatedAt: now });
      return null;
    },
    size: () => buckets.size,
  };
}

// The caller's address. The host's edge sets these headers from the
// connection and overwrites a client-supplied value (Vercel does, for both),
// so they name the caller, never this server. Absent (a local `next dev`),
// every caller shares one bucket.
export function clientAddress(req: Request): string {
  const real = req.headers.get("x-real-ip")?.trim();
  if (real) return real;
  const forwarded = req.headers.get("x-forwarded-for")?.split(",")[0]?.trim();
  return forwarded || "unknown";
}
