import { type Address, address } from "@solana/kit";
import { mainnet } from "@solana/rpc-types";

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
// see .env.example.
const primaryRpc = process.env.NEXT_PUBLIC_PRIMARY_RPC_URL?.trim();
const primaryWs = process.env.NEXT_PUBLIC_PRIMARY_WS_URL?.trim();
if (!primaryRpc !== !primaryWs) {
  throw new Error(
    "Set NEXT_PUBLIC_PRIMARY_RPC_URL and NEXT_PUBLIC_PRIMARY_WS_URL together, or neither.",
  );
}
export const PRIMARY_RPC_URL = primaryRpc ? mainnet(primaryRpc) : null;
export const PRIMARY_WS_URL = primaryWs || null;

// The endpoint a single-URL consumer (the MetaMask connector) should use.
export const PUBLIC_RPC_URL = PRIMARY_RPC_URL ?? FALLBACK_RPC_URL;

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

// External APIs. Defaults point at the dev/public endpoints we use today;
// override to swap in a staging server, a proxy route handler, or an
// alternative provider without editing hook code.
export const DFLOW_QUOTE_URL =
  process.env.NEXT_PUBLIC_DFLOW_QUOTE_URL?.trim() ||
  "https://dev-quote-api.dflow.net/quote";
export const DFLOW_ORDER_URL =
  process.env.NEXT_PUBLIC_DFLOW_ORDER_URL?.trim() ||
  "https://dev-quote-api.dflow.net/order";
export const JUPITER_SEARCH_URL =
  process.env.NEXT_PUBLIC_JUPITER_SEARCH_URL?.trim() ||
  "https://lite-api.jup.ag/tokens/v2/search";

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
