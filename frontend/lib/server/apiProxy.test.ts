import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DFLOW_ORDER_UPSTREAM,
  DFLOW_QUOTE_UPSTREAM,
  forward,
  JUPITER_SEARCH_UPSTREAM,
  type KeyedUpstream,
  type ProxyRoute,
  proxyFlags,
  serverKey,
} from "./apiProxy";

// A route whose check accepts any query unchanged, so these tests exercise
// forwarding alone; the real checks are tested in proxyQuery.test.ts.
const passThrough = (upstream: KeyedUpstream): ProxyRoute => ({
  upstream,
  validate: async (params) => ({ ok: true, search: params.toString() }),
});

const KEY = "sk-test-0123456789";

type Seen = {
  url: string;
  headers: Record<string, string>;
  redirect: RequestRedirect | undefined;
};

// Stub the upstream fetch: record what the proxy sent, answer with `reply`.
const stubUpstream = (reply: () => Response): Seen[] => {
  const seen: Seen[] = [];
  vi.stubGlobal("fetch", async (input: URL, init?: RequestInit) => {
    seen.push({
      url: input.toString(),
      headers: { ...(init?.headers as Record<string, string>) },
      redirect: init?.redirect,
    });
    return reply();
  });
  return seen;
};

// An upstream that echoes the key back in a header, so the not-echoed
// assertions test the proxy's header allow-list rather than a stub that
// never mentions the key.
const ok = () =>
  new Response(JSON.stringify({ outAmount: "42" }), {
    status: 200,
    headers: {
      "content-type": "application/json",
      "x-ratelimit-limit": "60",
      "x-ratelimit-remaining": "59",
      "x-api-key": KEY,
      "set-cookie": "upstream=1",
    },
  });

const request = (query: string) =>
  new Request(`https://app.test/api/route?${query}`);

// Everything a client can read off the response.
const visible = async (res: Response) =>
  `${[...res.headers.entries()].join(";")}|${await res.text()}`;

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const PROXIES: [string, KeyedUpstream][] = [
  ["DFlow quote", DFLOW_QUOTE_UPSTREAM],
  ["DFlow order", DFLOW_ORDER_UPSTREAM],
  ["Jupiter search", JUPITER_SEARCH_UPSTREAM],
];

describe.each(PROXIES)("%s proxy", (_, upstream) => {
  it("sends the key upstream to the keyed endpoint and never echoes it", async () => {
    const seen = stubUpstream(ok);
    const res = await forward(
      passThrough(upstream),
      request("inputMint=A&amount=7"),
      {
        [upstream.keyVar]: KEY,
      },
    );

    expect(seen).toHaveLength(1);
    expect(seen[0]?.url).toBe(`${upstream.keyedUrl}?inputMint=A&amount=7`);
    expect(seen[0]?.headers["x-api-key"]).toBe(KEY);
    // A redirect would carry the header to another host; it is refused.
    expect(seen[0]?.redirect).toBe("error");
    expect(res.status).toBe(200);
    expect(await visible(res)).not.toContain(KEY);
  });

  it("forwards keyless to the public endpoint when the key is unset", async () => {
    const seen = stubUpstream(ok);
    const res = await forward(passThrough(upstream), request("query=M"), {});

    expect(seen[0]?.url).toBe(`${upstream.keylessUrl}?query=M`);
    expect(seen[0]?.headers).not.toHaveProperty("x-api-key");
    expect(res.status).toBe(200);
  });

  it("refuses an unresolved 1Password reference without calling upstream", async () => {
    const seen = stubUpstream(ok);
    const ref = "op://vault/item/credential";
    const res = await forward(passThrough(upstream), request("query=M"), {
      [upstream.keyVar]: ref,
    });

    expect(seen).toHaveLength(0);
    expect(res.status).toBe(500);
    const body = await res.text();
    expect(body).toContain(upstream.keyVar);
    expect(body).not.toContain(ref);
  });
});

describe("forward", () => {
  it("passes a 429 through with its rate-limit headers and drops the rest", async () => {
    stubUpstream(
      () =>
        new Response('{"msg":"rate limited"}', {
          status: 429,
          headers: {
            "content-type": "application/json; charset=utf-8",
            "x-ratelimit-limit": "60",
            "x-ratelimit-remaining": "0",
            "set-cookie": "upstream=1",
          },
        }),
    );
    const res = await forward(
      passThrough(DFLOW_QUOTE_UPSTREAM),
      request("amount=1"),
      { DFLOW_API_KEY: KEY },
    );

    expect(res.status).toBe(429);
    expect(res.headers.get("x-ratelimit-remaining")).toBe("0");
    expect(res.headers.get("set-cookie")).toBeNull();
    expect(await res.json()).toEqual({ msg: "rate limited" });
  });

  it("adds nosniff to a JSON upstream response", async () => {
    stubUpstream(ok);
    const res = await forward(
      passThrough(JUPITER_SEARCH_UPSTREAM),
      request("query=M"),
      {},
    );

    expect(res.headers.get("content-type")).toBe("application/json");
    expect(res.headers.get("x-content-type-options")).toBe("nosniff");
  });

  it("passes a bodiless status through without a body", async () => {
    stubUpstream(() => new Response(null, { status: 204 }));
    const res = await forward(
      passThrough(JUPITER_SEARCH_UPSTREAM),
      request("query=M"),
      {},
    );

    expect(res.status).toBe(204);
    expect(await res.text()).toBe("");
  });

  it("checks the query before reading the key", async () => {
    stubUpstream(ok);
    const route: ProxyRoute = {
      upstream: JUPITER_SEARCH_UPSTREAM,
      validate: async () => ({ ok: false, msg: "bad query" }),
    };
    const res = await forward(route, request("query=M"), {
      JUPITER_API_KEY: "op://vault/item/credential",
    });

    expect(res.status).toBe(400);
  });

  it("replaces a non-JSON upstream body under the same status", async () => {
    const page = "<script>alert(1)</script>";
    stubUpstream(
      () =>
        new Response(page, {
          status: 429,
          headers: { "content-type": "text/html" },
        }),
    );
    const res = await forward(
      passThrough(JUPITER_SEARCH_UPSTREAM),
      request("query=M"),
      {},
    );

    expect(res.status).toBe(429);
    expect(res.headers.get("content-type")).toBe("application/json");
    const body = await res.text();
    expect(body).not.toContain(page);
    expect(JSON.parse(body).msg).toContain("non-JSON");
  });

  it("forwards the query the route's check rebuilt, not the caller's", async () => {
    const seen = stubUpstream(ok);
    const route: ProxyRoute = {
      upstream: JUPITER_SEARCH_UPSTREAM,
      validate: async () => ({ ok: true, search: "query=REBUILT" }),
    };
    await forward(route, request("query=M&extra=1"), {});

    expect(seen[0]?.url).toBe(
      `${JUPITER_SEARCH_UPSTREAM.keylessUrl}?query=REBUILT`,
    );
  });

  it("answers a refused query with a 400 and never calls upstream", async () => {
    const seen = stubUpstream(ok);
    const route: ProxyRoute = {
      upstream: JUPITER_SEARCH_UPSTREAM,
      validate: async () => ({ ok: false, msg: "bad query" }),
    };
    const res = await forward(route, request("query=M"), {
      JUPITER_API_KEY: KEY,
    });

    expect(seen).toHaveLength(0);
    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ msg: "bad query" });
  });

  it.each([
    ["HEAD", {}, 405],
    ["GET", { "sec-fetch-site": "cross-site" }, 403],
    ["GET", { "sec-fetch-site": "same-site" }, 403],
  ])("refuses %s with %o without calling upstream", async (method, headers, status) => {
    const seen = stubUpstream(ok);
    const res = await forward(
      passThrough(JUPITER_SEARCH_UPSTREAM),
      new Request("https://app.test/api/route?query=M", { method, headers }),
      { JUPITER_API_KEY: KEY },
    );

    expect(seen).toHaveLength(0);
    expect(res.status).toBe(status);
  });

  it("forwards a same-origin browser request", async () => {
    const seen = stubUpstream(ok);
    const res = await forward(
      passThrough(JUPITER_SEARCH_UPSTREAM),
      new Request("https://app.test/api/route?query=M", {
        headers: { "sec-fetch-site": "same-origin" },
      }),
      {},
    );

    expect(seen).toHaveLength(1);
    expect(res.status).toBe(200);
  });

  it("answers a network failure with a 502 naming the host only", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("fetch failed");
    });
    const res = await forward(
      passThrough(DFLOW_ORDER_UPSTREAM),
      request("amount=1"),
      { DFLOW_API_KEY: KEY },
    );

    expect(res.status).toBe(502);
    const body = await res.text();
    expect(body).toContain("refused redirect reaching quote-api.dflow.net");
    expect(body).not.toContain(KEY);
  });
});

describe("serverKey", () => {
  it("treats blank as unset and trims a value", () => {
    expect(serverKey("K", {})).toBeNull();
    expect(serverKey("K", { K: "  " })).toBeNull();
    expect(serverKey("K", { K: ` ${KEY} ` })).toBe(KEY);
  });
});

describe("proxyFlags", () => {
  it("routes the browser through a proxy only when its key is set", () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    expect(proxyFlags({})).toEqual({
      NEXT_PUBLIC_DFLOW_PROXY: "",
      NEXT_PUBLIC_JUPITER_PROXY: "",
    });
    expect(proxyFlags({ DFLOW_API_KEY: KEY })).toEqual({
      NEXT_PUBLIC_DFLOW_PROXY: "1",
      NEXT_PUBLIC_JUPITER_PROXY: "",
    });
    expect(proxyFlags({ JUPITER_API_KEY: KEY })).toEqual({
      NEXT_PUBLIC_DFLOW_PROXY: "",
      NEXT_PUBLIC_JUPITER_PROXY: "1",
    });
  });

  it("logs hostnames, never the key", () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    proxyFlags({ DFLOW_API_KEY: KEY, JUPITER_API_KEY: KEY });
    const lines = log.mock.calls.flat().join("\n");
    expect(lines).toContain(
      "keyed, via the route handler to quote-api.dflow.net",
    );
    expect(lines).toContain("api.jup.ag");
    expect(lines).not.toContain(KEY);
  });

  it("refuses an unresolved reference at startup", () => {
    vi.spyOn(console, "log").mockImplementation(() => {});
    expect(() =>
      proxyFlags({ JUPITER_API_KEY: "op://vault/item/credential" }),
    ).toThrow(/JUPITER_API_KEY/);
  });
});
