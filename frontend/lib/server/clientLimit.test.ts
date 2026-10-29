import { describe, expect, it } from "vitest";
import { clientAddress, clientLimiter, MAX_CLIENTS } from "./clientLimit";

describe("clientLimiter", () => {
  it("admits a full bucket back to back, then names the wait", () => {
    const limiter = clientLimiter({ capacity: 3, refillPerSec: 0.5 });
    expect([0, 0, 0].map(() => limiter.take("a", 0))).toEqual([
      null,
      null,
      null,
    ]);
    // One token back takes 2 s at 0.5/s.
    expect(limiter.take("a", 0)).toBe(2);
    expect(limiter.take("a", 1_000)).toBe(1);
    expect(limiter.take("a", 2_000)).toBeNull();
  });

  it("rounds a sub-second wait up to one second", () => {
    const limiter = clientLimiter({ capacity: 1, refillPerSec: 1 });
    expect(limiter.take("a", 0)).toBeNull();
    expect(limiter.take("a", 900)).toBe(1);
  });

  it("refills no further than capacity", () => {
    const limiter = clientLimiter({ capacity: 2, refillPerSec: 1 });
    limiter.take("a", 0);
    expect(limiter.take("a", 60_000)).toBeNull();
    expect(limiter.take("a", 60_000)).toBeNull();
    expect(limiter.take("a", 60_000)).toBe(1);
  });

  it("credits no refill twice when the clock steps back", () => {
    const limiter = clientLimiter({ capacity: 2, refillPerSec: 1 });
    expect(limiter.take("a", 10_000)).toBeNull();
    // Admitted on a clock stepped back to 9 s: the bucket's clock stays at
    // 10 s, so half a second later there is half a token, not one and a half.
    expect(limiter.take("a", 9_000)).toBeNull();
    expect(limiter.take("a", 10_500)).toBe(1);
  });

  it("keeps a refused caller off the eviction front", () => {
    const limiter = clientLimiter({ capacity: 1, refillPerSec: 1 });
    limiter.take("hammer", 0);
    for (let i = 0; i < MAX_CLIENTS - 1; i++) limiter.take(`c${i}`, 0);
    // Refused, so moved to the back: the flood's eviction takes c0.. first.
    expect(limiter.take("hammer", 0)).toBe(1);
    limiter.take("next", 0);
    expect(limiter.take("hammer", 0)).toBe(1);
  });

  it("keeps one caller's spend off another's bucket", () => {
    const limiter = clientLimiter({ capacity: 1, refillPerSec: 1 });
    expect(limiter.take("a", 0)).toBeNull();
    expect(limiter.take("a", 0)).toBe(1);
    expect(limiter.take("b", 0)).toBeNull();
  });

  it("drops refilled callers at the cap and keeps a spent one", () => {
    const limiter = clientLimiter({ capacity: 1, refillPerSec: 1 });
    for (let i = 0; i < MAX_CLIENTS - 1; i++) limiter.take(`c${i}`, 0);
    limiter.take("spent", 5_000);
    expect(limiter.size()).toBe(MAX_CLIENTS);
    // At the cap: every c* has refilled by now, "spent" has not.
    limiter.take("next", 5_000);
    expect(limiter.size()).toBe(2);
    expect(limiter.take("spent", 5_000)).toBe(1);
  });

  it("evicts the least recently seen when no caller has refilled", () => {
    const limiter = clientLimiter({ capacity: 1, refillPerSec: 1 });
    for (let i = 0; i <= MAX_CLIENTS; i++) limiter.take(`c${i}`, 0);
    // Evicted to 90% of the cap, then the new caller added.
    expect(limiter.size()).toBe(MAX_CLIENTS * 0.9 + 1);
    // The newest are still spent; c0 was evicted and reads as fresh.
    expect(limiter.take(`c${MAX_CLIENTS - 1}`, 0)).toBe(1);
    expect(limiter.take("c0", 0)).toBeNull();
  });
});

describe("clientAddress", () => {
  const req = (headers: Record<string, string>) =>
    new Request("https://app.test/api/route", { headers });

  it("prefers x-real-ip, then the first x-forwarded-for hop", () => {
    expect(
      clientAddress(
        req({ "x-real-ip": "1.1.1.1", "x-forwarded-for": "2.2.2.2" }),
      ),
    ).toBe("1.1.1.1");
    expect(
      clientAddress(req({ "x-forwarded-for": " 2.2.2.2 , 10.0.0.1" })),
    ).toBe("2.2.2.2");
  });

  it("keys an IPv6 caller on its /64", () => {
    const key = (ip: string) => clientAddress(req({ "x-real-ip": ip }));
    expect(key("2001:db8:1:2:aaaa::1")).toBe("2001:db8:1:2::/64");
    expect(key("2001:0db8:0001:0002:ffff:ffff:ffff:ffff")).toBe(
      "2001:db8:1:2::/64",
    );
    expect(key("2001:db8::5")).toBe("2001:db8:0:0::/64");
    expect(key("fe80::1%eth0")).toBe("fe80:0:0:0::/64");
    expect(key("2001:db8:1:3::1")).not.toBe(key("2001:db8:1:2::1"));
    expect(key("::ffff:1.2.3.4")).toBe("1.2.3.4");
    expect(() => key("1:2:3:4:5:6:7:8::9")).not.toThrow();
  });

  it("shares one bucket when no address is known", () => {
    expect(clientAddress(req({}))).toBe("unknown");
  });
});
