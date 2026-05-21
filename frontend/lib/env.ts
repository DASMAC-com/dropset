import { type Address, address } from "@solana/kit";
import { mainnet } from "@solana/rpc-types";

const DEFAULT_RPC = "https://api.mainnet-beta.solana.com";
const DEFAULT_WS = "wss://api.mainnet-beta.solana.com";

export const PUBLIC_RPC_URL = mainnet(
  process.env.NEXT_PUBLIC_RPC_URL ?? DEFAULT_RPC,
);
export const PUBLIC_WS_URL = process.env.NEXT_PUBLIC_WS_URL ?? DEFAULT_WS;

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
