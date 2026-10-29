// Server-only: the query checks the route handlers under app/api/ run before
// lib/server/apiProxy.ts attaches a key. Each route accepts exactly the
// parameters the app's own callers send (lib/hooks/useUsdQuote.ts for Jupiter;
// the SDK's fetchDflowQuote and lib/dflow/dflowSwap.ts for DFlow), with each
// value checked, and the query sent upstream is rebuilt from those alone. An
// unknown or repeated parameter is refused rather than dropped, so a caller
// change that adds one fails loudly in keyed mode instead of silently losing
// the parameter.

import { address, isAddress } from "@solana/kit";
import {
  findAssociatedTokenPda,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";
import {
  DFLOW_PROXY_ORDER_LIMIT,
  DFLOW_PROXY_QUOTE_LIMIT,
} from "../data/timings";
import {
  DFLOW_ORDER_UPSTREAM,
  DFLOW_QUOTE_UPSTREAM,
  type Env,
  JUPITER_SEARCH_UPSTREAM,
  type ProxyRoute,
  type QueryCheck,
  type QueryValidator,
} from "./apiProxy";
import { clientLimiter } from "./clientLimit";

const refuse = (msg: string): QueryCheck => ({ ok: false, msg });

// Jupiter's Tokens API caps a search at 100 mints, and useUsdQuote.ts chunks
// at the same 100.
export const JUPITER_SEARCH_MAX_MINTS = 100;

// `query` only: a comma-joined list of 1 to 100 mint addresses. Free text
// would spend the keyed quota on searches the app never makes. Any mint is
// allowed, not only the roster's: the check bounds the request's shape, and a
// roster pin would refuse a mint a newer client asks for before this server is
// redeployed.
export const jupiterSearchQuery: QueryValidator = async (params) => {
  const names = new Set(params.keys());
  if (names.size !== 1 || !names.has("query")) {
    return refuse("Only a query parameter is accepted");
  }
  const all = params.getAll("query");
  const entries = all.length === 1 ? (all[0] ?? "").split(",") : [];
  if (
    entries.length < 1 ||
    entries.length > JUPITER_SEARCH_MAX_MINTS ||
    !entries.every((mint) => isAddress(mint))
  ) {
    return refuse(
      `query must be 1 to ${JUPITER_SEARCH_MAX_MINTS} comma-separated mint addresses`,
    );
  }
  // Commas left literal, as the app's callers send them.
  return { ok: true, search: `query=${entries.join(",")}` };
};

type Rule = { check: (value: string) => boolean; optional?: true };

const U64_MAX = (1n << 64n) - 1n;
const pubkey: Rule = { check: (v) => isAddress(v) };
const atomicAmount: Rule = {
  check: (v) => /^[1-9]\d{0,19}$/.test(v) && BigInt(v) <= U64_MAX,
};
// Shape only: a custom percent is bounded below but not above in the UI
// (lib/format/input.ts parseSlippagePercent), so any sane value is left to
// DFlow, as before the proxy checked anything; the 7-digit cap only bounds the
// string's length.
const slippage: Rule = {
  check: (v) => v === "auto" || /^\d{1,7}$/.test(v),
};
const fixed = (value: string): Rule => ({ check: (v) => v === value });
// The fee params' values are checked against the server's fee by `feeCheck`.
const feeParam: Rule = { check: () => true, optional: true };

// The query rebuilt from `rules`' parameters, in their order, or the first
// reason to refuse: an unknown or repeated parameter, a missing required one,
// or a value its rule rejects.
function rebuild(
  params: URLSearchParams,
  rules: Record<string, Rule>,
): QueryCheck {
  for (const name of new Set(params.keys())) {
    if (!Object.hasOwn(rules, name)) {
      return refuse(`Unexpected parameter ${name}`);
    }
    if (params.getAll(name).length > 1) {
      return refuse(`Repeated parameter ${name}`);
    }
  }
  const out = new URLSearchParams();
  for (const [name, rule] of Object.entries(rules)) {
    const value = params.get(name);
    if (value === null) {
      if (rule.optional) continue;
      return refuse(`Missing parameter ${name}`);
    }
    if (!rule.check(value)) return refuse(`Invalid parameter ${name}`);
    out.set(name, value);
  }
  return { ok: true, search: out.toString() };
}

// The deployment's platform fee, by lib/env.ts's bps and wallet rules (that
// module is the browser's and throws without the RPC variables, so it is not
// imported here); null when none is configured. An invalid wallet reads as
// none here, where env.ts throws, so a declared fee is refused.
function serverFee(env: Env): { bps: number; wallet: string } | null {
  const bps = Number.parseInt(env.NEXT_PUBLIC_PLATFORM_FEE_BPS ?? "", 10);
  const wallet = env.NEXT_PUBLIC_PLATFORM_FEE_WALLET?.trim();
  return Number.isFinite(bps) && bps > 0 && wallet && isAddress(wallet)
    ? { bps, wallet }
    : null;
}

// A declared fee must be this deployment's own: its bps, the output-mint
// mode, and the fee wallet's associated token account for the output mint
// under either token program. Anything else would let a caller relay
// fee-bearing orders to an account of its choosing under the server's key and
// attribution. No fee declared passes; the app omits it when the fee vault is
// missing (lib/dflow/feeVault.ts).
async function feeCheck(
  params: URLSearchParams,
  env: Env,
): Promise<string | null> {
  const bps = params.get("platformFeeBps");
  const account = params.get("feeAccount");
  const mode = params.get("platformFeeMode");
  if (bps === null && account === null && mode === null) return null;

  const fee = serverFee(env);
  if (!fee) return "No platform fee is configured";
  if (bps !== String(fee.bps) || mode !== "outputMint" || account === null) {
    return "The platform fee does not match this deployment's";
  }
  const mint = address(params.get("outputMint") ?? "");
  const owner = address(fee.wallet);
  for (const tokenProgram of [
    TOKEN_PROGRAM_ADDRESS,
    TOKEN_2022_PROGRAM_ADDRESS,
  ]) {
    const [ata] = await findAssociatedTokenPda({ owner, mint, tokenProgram });
    if (ata === account) return null;
  }
  return "feeAccount is not the fee wallet's account for outputMint";
}

const FEE_RULES: Record<string, Rule> = {
  platformFeeBps: feeParam,
  feeAccount: feeParam,
  platformFeeMode: feeParam,
};

const dflowQuery =
  (rules: Record<string, Rule>): QueryValidator =>
  async (params, env) => {
    const check = rebuild(params, { ...rules, ...FEE_RULES });
    if (!check.ok) return check;
    const feeError = await feeCheck(params, env);
    return feeError === null ? check : refuse(feeError);
  };

const QUOTE_RULES: Record<string, Rule> = {
  inputMint: pubkey,
  outputMint: pubkey,
  amount: atomicAmount,
  slippageBps: slippage,
};

export const dflowQuoteQuery = dflowQuery(QUOTE_RULES);
export const dflowOrderQuery = dflowQuery({
  ...QUOTE_RULES,
  userPublicKey: pubkey,
  allowAsyncExec: fixed("false"),
  dynamicComputeUnitLimit: fixed("true"),
});

// Each DFlow route holds its own buckets; Jupiter's search is unlimited, as
// its one caller batches mints and refreshes on a slow cadence.
export const DFLOW_QUOTE_ROUTE: ProxyRoute = {
  upstream: DFLOW_QUOTE_UPSTREAM,
  validate: dflowQuoteQuery,
  limiter: clientLimiter(DFLOW_PROXY_QUOTE_LIMIT),
};
export const DFLOW_ORDER_ROUTE: ProxyRoute = {
  upstream: DFLOW_ORDER_UPSTREAM,
  validate: dflowOrderQuery,
  limiter: clientLimiter(DFLOW_PROXY_ORDER_LIMIT),
};
export const JUPITER_SEARCH_ROUTE: ProxyRoute = {
  upstream: JUPITER_SEARCH_UPSTREAM,
  validate: jupiterSearchQuery,
};
