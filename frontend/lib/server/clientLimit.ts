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
    // request, refused ones included, so the first entries are the least
    // recently seen and a caller hammering past its limit is never the one
    // evicted (which would hand it a full bucket). Evicting to
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
      buckets.delete(client);
      if (prev && tokens < 1) {
        // Moved to the back unchanged: a refusal spends nothing.
        buckets.set(client, prev);
        return Math.max(1, Math.ceil((1 - tokens) / limit.refillPerSec));
      }
      if (buckets.size >= MAX_CLIENTS) prune(now);
      // Never rewound: a clock stepping back would otherwise credit the same
      // interval as refill twice once it caught up.
      const updatedAt = Math.max(prev?.updatedAt ?? now, now);
      buckets.set(client, { tokens: tokens - 1, updatedAt });
      return null;
    },
    size: () => buckets.size,
  };
}

// The caller's address, as the bucket key. The host's edge sets these headers
// from the connection and overwrites a client-supplied value (Vercel does,
// for both), so they name the caller, never this server. Absent (a local
// `next dev`), every caller shares one bucket.
export function clientAddress(req: Request): string {
  const real = req.headers.get("x-real-ip")?.trim();
  const forwarded = req.headers.get("x-forwarded-for")?.split(",")[0]?.trim();
  const address = real || forwarded;
  return address ? bucketKey(address) : "unknown";
}

// An IPv6 host is normally handed a whole /64, so keying on its full address
// would let it draw a fresh bucket per request by rotating the low bits; it
// is keyed on the /64 instead. An IPv4 address, IPv4-mapped IPv6 included,
// is its own key.
function bucketKey(address: string): string {
  if (!address.includes(":")) return address;
  if (address.includes(".")) return address.slice(address.lastIndexOf(":") + 1);
  const bare = address.replace(/^\[|\]$|%.*$/g, "").toLowerCase();
  const [head, tail] = bare.split("::");
  const left = head ? head.split(":") : [];
  const right = tail ? tail.split(":") : [];
  // Clamped so a malformed address with too many groups keys oddly rather
  // than throwing.
  const zeros = Math.max(0, 8 - left.length - right.length);
  const groups =
    tail === undefined ? left : [...left, ...Array(zeros).fill("0"), ...right];
  const prefix = groups
    .slice(0, 4)
    .map((g) => g.replace(/^0+(?=.)/, ""))
    .join(":");
  return `${prefix}::/64`;
}
