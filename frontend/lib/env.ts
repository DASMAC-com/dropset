import { type Address, address } from "@solana/kit";
import { mainnet } from "@solana/rpc-types";
import {
  DFLOW_ORDER_URLS,
  DFLOW_QUOTE_URLS,
  JUPITER_SEARCH_URLS,
} from "./upstreams";

function required(name: string, value: string | undefined): string {
  if (!value) {
    throw new Error(
      `Missing ${name}. Set it in .env.local (dev) or your hosting env (prod).`,
    );
  }
  return value;
}

// The standing free endpoint (PublicNode on mainnet, the local validator on
// localnet). Required: it serves every call when no primary is set, and is
// the fallback behind one that is.
export const FALLBACK_RPC_URL = mainnet(
  required("NEXT_PUBLIC_RPC_URL", process.env.NEXT_PUBLIC_RPC_URL),
);
export const FALLBACK_WS_URL = required(
  "NEXT_PUBLIC_WS_URL",
  process.env.NEXT_PUBLIC_WS_URL,
);

// The optional keyed primary (Helius for the demo). Deleting both from the
// hosting env returns a deploy to the standing endpoint alone. The key ships in
// the bundle, so it must be origin-restricted in the provider's dashboard;
// see .env.example. Only the app's own runtime uses it (lib/rpc.ts): a
// single-URL consumer such as the MetaMask connector keeps the standing
// endpoint, since it may call from an origin the restriction refuses and
// cannot fail over.
const primaryRpc = process.env.NEXT_PUBLIC_PRIMARY_RPC_URL?.trim();
const primaryWs = process.env.NEXT_PUBLIC_PRIMARY_WS_URL?.trim();
if (!primaryRpc !== !primaryWs) {
  throw new Error(
    "Set NEXT_PUBLIC_PRIMARY_RPC_URL and NEXT_PUBLIC_PRIMARY_WS_URL together, or neither.",
  );
}
export const PRIMARY_RPC_URL = primaryRpc ? mainnet(primaryRpc) : null;
export const PRIMARY_WS_URL = primaryWs || null;

// Which Solana cluster the app targets. Defaults to mainnet (production);
// set NEXT_PUBLIC_CLUSTER=localnet to point at a local validator. Localnet
// forces eCLOB-only routing (no DFlow aggregator, which only knows mainnet
// liquidity) and swaps in the mock localnet mints (currencies.localnet.json).
// The RPC/WS URLs above carry the actual endpoint — this flag only gates the
// cluster-conditional behavior, so keep them in sync (a localnet endpoint
// with CLUSTER=mainnet would route to DFlow against a chain it can't see).
export type Cluster = "localnet" | "mainnet";
export const CLUSTER: Cluster =
  process.env.NEXT_PUBLIC_CLUSTER?.trim() === "localnet"
    ? "localnet"
    : "mainnet";
export const IS_LOCALNET = CLUSTER === "localnet";

// External APIs. With a server-side key set (DFLOW_API_KEY, JUPITER_API_KEY),
// next.config.ts sets the matching *_PROXY flag and the browser calls the
// app's own route handler, which adds the key; without one it calls the
// keyless public endpoint directly, as before the handlers existed. An
// explicit URL override wins over both, for a staging server or an
// alternative provider. A route is relative: resolve it with `absoluteUrl`
// before handing it to `new URL`.
const DFLOW_PROXY = process.env.NEXT_PUBLIC_DFLOW_PROXY === "1";
const JUPITER_PROXY = process.env.NEXT_PUBLIC_JUPITER_PROXY === "1";
export const DFLOW_QUOTE_URL =
  process.env.NEXT_PUBLIC_DFLOW_QUOTE_URL?.trim() ||
  (DFLOW_PROXY ? "/api/dflow/quote" : DFLOW_QUOTE_URLS.keyless);
export const DFLOW_ORDER_URL =
  process.env.NEXT_PUBLIC_DFLOW_ORDER_URL?.trim() ||
  (DFLOW_PROXY ? "/api/dflow/order" : DFLOW_ORDER_URLS.keyless);
export const JUPITER_SEARCH_URL =
  process.env.NEXT_PUBLIC_JUPITER_SEARCH_URL?.trim() ||
  (JUPITER_PROXY ? "/api/jupiter/search" : JUPITER_SEARCH_URLS.keyless);

// Browser-only: an endpoint above as an absolute URL, against the page's own
// origin when it is a route.
export const absoluteUrl = (url: string): string =>
  new URL(url, window.location.href).toString();

// Max accounts per `getMultipleAccounts` call. Required, provider-specific
// (PublicNode caps at 10 on their free tier). With a primary set, a call can
// land on either provider, so this must fit the fallback's cap.
const parsedBatchSize = Number.parseInt(
  required(
    "NEXT_PUBLIC_GET_MULTIPLE_ACCOUNTS_BATCH_SIZE",
    process.env.NEXT_PUBLIC_GET_MULTIPLE_ACCOUNTS_BATCH_SIZE,
  ),
  10,
);
if (!Number.isFinite(parsedBatchSize) || parsedBatchSize < 0) {
  throw new Error(
    "NEXT_PUBLIC_GET_MULTIPLE_ACCOUNTS_BATCH_SIZE must be a non-negative integer (0 = no chunking, send all in one call)",
  );
}
export const GET_MULTIPLE_ACCOUNTS_BATCH_SIZE = parsedBatchSize;

// DFlow platform fee. Resolves to null when either side is missing/blank or
// the bps value isn't a positive integer — callers should gate on
// PLATFORM_FEE to decide whether to forward the fee params to /order. See
// .env.example for setup details (ATAs must be pre-created via
// `pnpm setup-fee-atas` before enabling).
export type PlatformFee = { bps: number; wallet: Address };
const parsedBps = Number.parseInt(
  process.env.NEXT_PUBLIC_PLATFORM_FEE_BPS ?? "",
  10,
);
const walletRaw = process.env.NEXT_PUBLIC_PLATFORM_FEE_WALLET?.trim();
export const PLATFORM_FEE: PlatformFee | null =
  Number.isFinite(parsedBps) && parsedBps > 0 && walletRaw
    ? { bps: parsedBps, wallet: address(walletRaw) }
    : null;
