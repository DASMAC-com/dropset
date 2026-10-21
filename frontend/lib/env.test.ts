import { afterEach, describe, expect, it, vi } from "vitest";

// lib/env reads process.env at module scope, so each case stubs the env and
// imports a fresh copy.
const load = async (vars: Record<string, string>) => {
  vi.resetModules();
  for (const [name, value] of Object.entries(vars)) vi.stubEnv(name, value);
  return import("./env");
};

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("external API endpoints", () => {
  it("keeps the browser on the public keyless endpoints with no proxy flag", async () => {
    const env = await load({
      NEXT_PUBLIC_DFLOW_PROXY: "",
      NEXT_PUBLIC_JUPITER_PROXY: "",
    });
    expect(env.DFLOW_QUOTE_URL).toBe("https://dev-quote-api.dflow.net/quote");
    expect(env.DFLOW_ORDER_URL).toBe("https://dev-quote-api.dflow.net/order");
    expect(env.JUPITER_SEARCH_URL).toBe(
      "https://lite-api.jup.ag/tokens/v2/search",
    );
  });

  it("routes the browser through the app's handlers when flagged", async () => {
    const env = await load({
      NEXT_PUBLIC_DFLOW_PROXY: "1",
      NEXT_PUBLIC_JUPITER_PROXY: "1",
    });
    expect(env.DFLOW_QUOTE_URL).toBe("/api/dflow/quote");
    expect(env.DFLOW_ORDER_URL).toBe("/api/dflow/order");
    expect(env.JUPITER_SEARCH_URL).toBe("/api/jupiter/search");
  });

  it("lets an explicit URL override win over the proxy", async () => {
    const env = await load({
      NEXT_PUBLIC_DFLOW_PROXY: "1",
      NEXT_PUBLIC_DFLOW_QUOTE_URL: "https://staging.test/quote",
    });
    expect(env.DFLOW_QUOTE_URL).toBe("https://staging.test/quote");
    expect(env.DFLOW_ORDER_URL).toBe("/api/dflow/order");
  });
});
