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

export const PUBLIC_RPC_URL = mainnet(
  required("NEXT_PUBLIC_RPC_URL", process.env.NEXT_PUBLIC_RPC_URL),
);
export const PUBLIC_WS_URL = required(
  "NEXT_PUBLIC_WS_URL",
  process.env.NEXT_PUBLIC_WS_URL,
);

// Max accounts per `getMultipleAccounts` call. Required, provider-specific
// (PublicNode caps at 10 on their free tier).
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
