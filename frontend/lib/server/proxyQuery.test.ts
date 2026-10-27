import { type Address, address } from "@solana/kit";
import {
  findAssociatedTokenPda,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";
import { afterEach, describe, expect, it, vi } from "vitest";
import { forward } from "./apiProxy";
import {
  DFLOW_ORDER_ROUTE,
  DFLOW_QUOTE_ROUTE,
  dflowOrderQuery,
  dflowQuoteQuery,
  JUPITER_SEARCH_MAX_MINTS,
  JUPITER_SEARCH_ROUTE,
  jupiterSearchQuery,
} from "./proxyQuery";

const USDC = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const USER = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
const FEE_WALLET = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
const FEE_ENV = {
  NEXT_PUBLIC_PLATFORM_FEE_BPS: "25",
  NEXT_PUBLIC_PLATFORM_FEE_WALLET: FEE_WALLET,
};

const params = (query: string) => new URLSearchParams(query);

const feeAta = async (
  mint: string,
  tokenProgram: Address = TOKEN_PROGRAM_ADDRESS,
) =>
  (
    await findAssociatedTokenPda({
      owner: address(FEE_WALLET),
      mint: address(mint),
      tokenProgram,
    })
  )[0];

// The query lib/dflow/dflowSwap.ts sends to /order, fee params optional.
const orderQuery = (fee = "") =>
  `inputMint=${USDC}&outputMint=${USDT}&amount=1000000&slippageBps=auto` +
  `&userPublicKey=${USER}&allowAsyncExec=false&dynamicComputeUnitLimit=true${fee}`;

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("jupiterSearchQuery", () => {
  it("accepts a comma-joined mint list and keeps the commas literal", async () => {
    expect(
      await jupiterSearchQuery(params(`query=${USDC},${USDT}`), {}),
    ).toEqual({
      ok: true,
      search: `query=${USDC},${USDT}`,
    });
  });

  it("accepts exactly the cap", async () => {
    const mints = Array(JUPITER_SEARCH_MAX_MINTS).fill(USDC).join(",");
    expect((await jupiterSearchQuery(params(`query=${mints}`), {})).ok).toBe(
      true,
    );
  });

  it.each([
    ["free text", "query=bonk"],
    ["an empty query", "query="],
    ["a missing query", ""],
    ["an empty entry", `query=${USDC},`],
    ["a non-base58 entry", `query=${USDC.replace("E", "0")}`],
    ["a repeated query", `query=${USDC}&query=${USDT}`],
    ["an extra parameter", `query=${USDC}&limit=5`],
    [
      `${JUPITER_SEARCH_MAX_MINTS + 1} entries`,
      `query=${Array(JUPITER_SEARCH_MAX_MINTS + 1)
        .fill(USDC)
        .join(",")}`,
    ],
  ])("refuses %s", async (_, query) => {
    expect((await jupiterSearchQuery(params(query), {})).ok).toBe(false);
  });
});

describe("dflowQuoteQuery", () => {
  it("accepts the SDK's quote query and rebuilds it", async () => {
    const query = `inputMint=${USDC}&outputMint=${USDT}&amount=5&slippageBps=50`;
    expect(await dflowQuoteQuery(params(query), {})).toEqual({
      ok: true,
      search: query,
    });
  });

  it.each([
    ["a missing amount", `inputMint=${USDC}&outputMint=${USDT}&slippageBps=50`],
    [
      "a fractional amount",
      `inputMint=${USDC}&outputMint=${USDT}&amount=1.5&slippageBps=50`,
    ],
    [
      "a zero amount",
      `inputMint=${USDC}&outputMint=${USDT}&amount=0&slippageBps=50`,
    ],
    [
      "an amount over u64",
      `inputMint=${USDC}&outputMint=${USDT}&amount=18446744073709551616&slippageBps=50`,
    ],
    [
      "slippage over 100%",
      `inputMint=${USDC}&outputMint=${USDT}&amount=5&slippageBps=10001`,
    ],
    ["a bad mint", `inputMint=bonk&outputMint=${USDT}&amount=5&slippageBps=50`],
    [
      "an order-only parameter",
      `inputMint=${USDC}&outputMint=${USDT}&amount=5&slippageBps=50&userPublicKey=${USER}`,
    ],
  ])("refuses %s", async (_, query) => {
    expect((await dflowQuoteQuery(params(query), {})).ok).toBe(false);
  });
});

describe("dflowOrderQuery", () => {
  it("accepts the app's order query without a fee", async () => {
    expect(await dflowOrderQuery(params(orderQuery()), {})).toEqual({
      ok: true,
      search: orderQuery(),
    });
  });

  it.each([
    ["Token", TOKEN_PROGRAM_ADDRESS],
    ["Token-2022", TOKEN_2022_PROGRAM_ADDRESS],
  ])("accepts the deployment's fee to its %s account", async (_, program) => {
    const fee = `&platformFeeBps=25&feeAccount=${await feeAta(USDT, program)}&platformFeeMode=outputMint`;
    expect((await dflowOrderQuery(params(orderQuery(fee)), FEE_ENV)).ok).toBe(
      true,
    );
  });

  it("refuses a fee when the deployment configures none", async () => {
    const fee = `&platformFeeBps=25&feeAccount=${await feeAta(USDT)}&platformFeeMode=outputMint`;
    expect((await dflowOrderQuery(params(orderQuery(fee)), {})).ok).toBe(false);
  });

  it("refuses a fee to an account the deployment does not own", async () => {
    const fee = `&platformFeeBps=25&feeAccount=${USER}&platformFeeMode=outputMint`;
    expect(await dflowOrderQuery(params(orderQuery(fee)), FEE_ENV)).toEqual({
      ok: false,
      msg: "feeAccount is not the fee wallet's account for outputMint",
    });
  });

  it("refuses the fee wallet's account for the input mint", async () => {
    const fee = `&platformFeeBps=25&feeAccount=${await feeAta(USDC)}&platformFeeMode=outputMint`;
    expect((await dflowOrderQuery(params(orderQuery(fee)), FEE_ENV)).ok).toBe(
      false,
    );
  });

  it.each([
    [
      "another bps",
      "&platformFeeBps=500&feeAccount=ATA&platformFeeMode=outputMint",
    ],
    [
      "another mode",
      "&platformFeeBps=25&feeAccount=ATA&platformFeeMode=inputMint",
    ],
    ["bps without an account", "&platformFeeBps=25&platformFeeMode=outputMint"],
  ])("refuses %s", async (_, template) => {
    const fee = template.replace("ATA", await feeAta(USDT));
    expect((await dflowOrderQuery(params(orderQuery(fee)), FEE_ENV)).ok).toBe(
      false,
    );
  });

  it.each([
    [
      "async execution",
      orderQuery().replace("allowAsyncExec=false", "allowAsyncExec=true"),
    ],
    ["a missing user", orderQuery().replace(`&userPublicKey=${USER}`, "")],
    ["an unknown parameter", `${orderQuery()}&referralAccount=${USER}`],
  ])("refuses %s", async (_, query) => {
    expect((await dflowOrderQuery(params(query), {})).ok).toBe(false);
  });
});

describe("the routes", () => {
  it.each([
    ["Jupiter search", JUPITER_SEARCH_ROUTE, "query=bonk"],
    ["DFlow quote", DFLOW_QUOTE_ROUTE, "amount=1"],
    ["DFlow order", DFLOW_ORDER_ROUTE, `${orderQuery()}&platformFeeBps=500`],
  ])("%s refuses a bad query before the key reaches upstream", async (_, route, query) => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const res = await forward(
      route,
      new Request(`https://app.test/api?${query}`),
      {
        [route.upstream.keyVar]: "sk-test",
      },
    );

    expect(res.status).toBe(400);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("forwards a valid mint list to Jupiter with the key", async () => {
    const fetch = vi.fn(
      async (_input: URL) =>
        new Response("[]", { headers: { "content-type": "application/json" } }),
    );
    vi.stubGlobal("fetch", fetch);
    const res = await forward(
      JUPITER_SEARCH_ROUTE,
      new Request(`https://app.test/api?query=${USDC},${USDT}`),
      { JUPITER_API_KEY: "sk-test" },
    );

    expect(res.status).toBe(200);
    expect(fetch).toHaveBeenCalledOnce();
    expect(String(fetch.mock.calls[0]?.[0])).toBe(
      `${JUPITER_SEARCH_ROUTE.upstream.keyedUrl}?query=${USDC},${USDT}`,
    );
  });
});
