// cspell:word publicnode
import { describe, expect, it } from "vitest";

import { connectSources, securityHeaders } from "./securityHeaders";

const header = (headers: { key: string; value: string }[], key: string) =>
  headers.find((h) => h.key === key)?.value;

describe("security headers", () => {
  it("enforces framing, sniffing and referrer policy", () => {
    const headers = securityHeaders({}, { dev: false });
    expect(header(headers, "Content-Security-Policy")).toContain(
      "frame-ancestors 'none'",
    );
    expect(header(headers, "X-Frame-Options")).toBe("DENY");
    expect(header(headers, "X-Content-Type-Options")).toBe("nosniff");
    expect(header(headers, "Referrer-Policy")).toBe(
      "strict-origin-when-cross-origin",
    );
  });

  it("allows eval only in dev", () => {
    const policy = (dev: boolean) =>
      header(
        securityHeaders({}, { dev }),
        "Content-Security-Policy-Report-Only",
      );
    expect(policy(true)).toContain("'unsafe-eval'");
    expect(policy(false)).not.toContain("'unsafe-eval'");
  });

  it("lists the RPC, WS and API origins, never a key", () => {
    const sources = connectSources({
      NEXT_PUBLIC_RPC_URL: "https://solana-rpc.publicnode.com",
      NEXT_PUBLIC_WS_URL: "wss://solana-rpc.publicnode.com",
      NEXT_PUBLIC_PRIMARY_RPC_URL:
        "https://mainnet.helius-rpc.com/?api-key=secret-key",
      NEXT_PUBLIC_PRIMARY_WS_URL:
        "wss://mainnet.helius-rpc.com/?api-key=secret-key",
      NEXT_PUBLIC_DFLOW_QUOTE_URL: "/api/dflow/quote",
    });
    expect(sources).toEqual([
      "https://solana-rpc.publicnode.com",
      "wss://solana-rpc.publicnode.com",
      "https://mainnet.helius-rpc.com",
      "wss://mainnet.helius-rpc.com",
      "https://dev-quote-api.dflow.net",
      "https://lite-api.jup.ag",
    ]);
    expect(sources.join(" ")).not.toContain("secret-key");
  });

  it("puts every connect source in the report-only policy", () => {
    const env = { NEXT_PUBLIC_RPC_URL: "https://rpc.test" };
    const policy = header(
      securityHeaders(env, { dev: false }),
      "Content-Security-Policy-Report-Only",
    );
    expect(policy).toContain(
      `connect-src 'self' ${connectSources(env).join(" ")}`,
    );
  });

  it("keeps a local validator's port", () => {
    expect(
      connectSources({
        NEXT_PUBLIC_RPC_URL: "http://127.0.0.1:8899",
        NEXT_PUBLIC_WS_URL: "ws://127.0.0.1:8900",
      }).slice(0, 2),
    ).toEqual(["http://127.0.0.1:8899", "ws://127.0.0.1:8900"]);
  });
});
