import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ALL_STABLECOIN_MINTS } from "../data/currencies";
import { TOKEN_INFO_TICK_MAX_AGE_MS, TOKEN_INFO_TTL_MS } from "../data/timings";

vi.mock("../env", () => ({
  JUPITER_SEARCH_URL: "https://jupiter.test/search",
}));

// Each page's mount, replayed as the calls its components make in one effect
// pass (children first): the swap page's two token rows and the swap panel
// each read a pair price, then the panel warms the pair; every /currencies
// row reads its own mint, then the page warms the catalogue. /vaults mounts
// no Jupiter consumer, so it has no sequence to replay.

const [FROM = "", TO = ""] = ALL_STABLECOIN_MINTS;

// The module caches at module scope, so each case imports a fresh copy.
const load = async () => {
  vi.resetModules();
  return import("./useUsdQuote");
};

const queried = (fetchMock: ReturnType<typeof vi.fn>): string[][] =>
  fetchMock.mock.calls.map(([url]) =>
    (new URL(String(url)).searchParams.get("query") ?? "").split(","),
  );

let now = 1_000_000;
let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  now = 1_000_000;
  vi.spyOn(Date, "now").mockImplementation(() => now);
  fetchMock = vi.fn(async (url: string) => {
    const mints = (new URL(url).searchParams.get("query") ?? "").split(",");
    const rows = mints.map((id) => ({ id, usdPrice: 1 }));
    return new Response(JSON.stringify(rows));
  });
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Jupiter requests per page", () => {
  it("fetches only the swap pair, in one call, on the swap page", async () => {
    const m = await load();
    const reads = [
      m.requestTokenInfo(FROM),
      m.requestTokenInfo(TO),
      m.requestTokenInfo(FROM),
    ];
    await Promise.all([...reads, m.prefetchAllTokenInfo([FROM, TO])]);
    expect(queried(fetchMock)).toEqual([[FROM, TO]]);
  });

  it("fetches the catalogue in one call per 100 mints on /currencies", async () => {
    const m = await load();
    const reads = ALL_STABLECOIN_MINTS.map(m.requestTokenInfo);
    await Promise.all([...reads, m.prefetchAllTokenInfo(ALL_STABLECOIN_MINTS)]);
    const calls = queried(fetchMock);
    expect(calls).toHaveLength(Math.ceil(ALL_STABLECOIN_MINTS.length / 100));
    expect(calls.flat().sort()).toEqual([...ALL_STABLECOIN_MINTS].sort());
  });

  it("makes no request when navigating back within the TTL", async () => {
    const m = await load();
    await m.prefetchAllTokenInfo([FROM, TO]);
    now += TOKEN_INFO_TTL_MS - 1;
    await m.prefetchAllTokenInfo([FROM, TO]);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("fetches the pair again on a refresh tick past the tick age", async () => {
    const m = await load();
    await m.prefetchAllTokenInfo([FROM, TO]);
    now += TOKEN_INFO_TICK_MAX_AGE_MS;
    await m.prefetchAllTokenInfo([FROM, TO], TOKEN_INFO_TICK_MAX_AGE_MS);
    expect(queried(fetchMock)).toEqual([
      [FROM, TO],
      [FROM, TO],
    ]);
  });

  it("caches nothing from a failed call, so the next mount retries", async () => {
    const m = await load();
    fetchMock.mockResolvedValueOnce(new Response("", { status: 429 }));
    expect(await m.requestTokenInfo(FROM)).toBeNull();
    expect((await m.requestTokenInfo(FROM))?.usdPrice).toBe(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});
