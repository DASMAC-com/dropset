/**
 * DFlow aggregator client — the off-venue half of the router.
 *
 * Owns the DFlow wire contract: the `/quote` request (including the
 * platform-fee parameters), error extraction from a non-2xx body, and
 * validation of the response before any of it reaches a caller. The swap
 * (`/order`) path is deliberately **not** here — this module exists so the
 * router can *price* an aggregator route alongside our own book.
 *
 * Talks over `fetch`, so it runs in a browser or in Node ≥ 18 with no
 * additional dependency.
 */

import { type Address, fetchEncodedAccount } from '@solana/kit';
import { findAssociatedTokenPda } from '@solana-program/token';
import type { AccountRpc } from './route';

/** Why a DFlow call failed. */
export type DflowErrorKind =
  /** `fetch` threw — offline, DNS, CORS, or an aborted request. */
  | 'network'
  /** Non-2xx response (other than 429), or a body we could not validate. */
  | 'api'
  /** HTTP 429 — the caller should back off before retrying. */
  | 'rateLimited';

export class DflowError extends Error {
  readonly kind: DflowErrorKind;
  readonly httpStatus?: number;
  /** DFlow's machine-readable error code, when the body carried one. */
  readonly code?: string;
  constructor(
    message: string,
    kind: DflowErrorKind,
    httpStatus?: number,
    code?: string,
  ) {
    super(message);
    this.name = 'DflowError';
    this.kind = kind;
    this.httpStatus = httpStatus;
    this.code = code;
  }
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null;
const isString = (v: unknown): v is string => typeof v === 'string';
const isNumber = (v: unknown): v is number =>
  typeof v === 'number' && Number.isFinite(v);

/**
 * Coerce a decimal-string amount into a bigint. `BigInt()` throws on any
 * non-integer input (scientific notation, decimal points, stray characters),
 * which we surface as one typed reason. Amounts are unsigned atomic figures,
 * so a negative is rejected at the boundary rather than left for each consumer
 * to re-check. The message names the field but never echoes the raw value — a
 * malformed upstream body could carry data we don't want surfaced.
 */
function parseAmount(value: unknown, field: string): bigint {
  if (!isString(value)) {
    throw new DflowError(`${field} missing or not a string`, 'api');
  }
  let parsed: bigint;
  try {
    parsed = BigInt(value);
  } catch {
    throw new DflowError(`${field} is not a valid integer`, 'api');
  }
  if (parsed < 0n) {
    throw new DflowError(`${field} must be non-negative`, 'api');
  }
  return parsed;
}

const MAX_RAW_BODY_PREVIEW = 200;

/** A DFlow error body — both `/quote` and `/order` wrap failures this way. */
type DflowErrorBody = { code?: string; msg?: string };

export type DflowApiErrorInfo = { message: string; code: string | null };

/**
 * Extract a human-readable message from a non-2xx DFlow response. Falls back
 * to the status plus a truncated raw body, so a transient HTML 502 page
 * surfaces as `HTTP 502: <!DOCTYPE…` rather than a bare `HTTP 502`.
 */
export async function extractDflowApiError(
  res: Response,
): Promise<DflowApiErrorInfo> {
  let bodyText: string | null = null;
  try {
    bodyText = await res.text();
  } catch {
    return { message: `HTTP ${res.status}`, code: null };
  }
  if (!bodyText) return { message: `HTTP ${res.status}`, code: null };
  try {
    const body = JSON.parse(bodyText) as DflowErrorBody;
    if (isString(body?.msg) && body.msg.length > 0) {
      return { message: body.msg, code: isString(body.code) ? body.code : null };
    }
    if (isString(body?.code) && body.code.length > 0) {
      return { message: `${body.code} (HTTP ${res.status})`, code: body.code };
    }
    return { message: preview(res.status, bodyText), code: null };
  } catch {
    // Not JSON at all — an HTML error page or a proxy's plain-text body. This
    // is exactly the case worth previewing, so include it rather than
    // collapsing every such response to an undiagnosable status line.
    return { message: preview(res.status, bodyText), code: null };
  }
}

/** `HTTP <status>: <truncated body>` — enough to tell an outage from an API error. */
const preview = (status: number, body: string): string =>
  `HTTP ${status}: ${body.slice(0, MAX_RAW_BODY_PREVIEW)}`;

/** The platform fee an integrator wants to charge on aggregator routes. */
export type PlatformFeeConfig = {
  /** Fee in basis points. */
  bps: number;
  /** Wallet that collects the fee. Its ATA for the fee mint must already exist. */
  wallet: Address;
};

/** A platform fee that is actually chargeable — the fee ATA was found on-chain. */
export type ResolvedPlatformFee = { bps: number; feeAccount: Address };

/**
 * Resolve the platform fee for an output mint, or `null` when it must not be
 * declared.
 *
 * DFlow requires `feeAccount` to be a token account that **already exists**,
 * and it factors a declared `platformFeeBps` into the slippage budget *even
 * when the fee goes uncollected* — so advertising a fee whose vault is missing
 * both breaks `/order` and prices the user worse on `/quote`. Returning `null`
 * for a missing vault therefore skips the fee rather than breaking the route.
 *
 * Costs one `getAccountInfo` (plus one more when `tokenProgram` is omitted and
 * has to be read from the mint). Results are not cached here — a caller
 * quoting on a timer should cache per mint and pass the result to
 * {@link fetchDflowQuote}.
 */
export async function resolvePlatformFee(
  rpc: AccountRpc,
  input: {
    fee: PlatformFeeConfig | null;
    /** The mint the fee is collected in — the output mint under the default fee mode. */
    mint: Address;
    /** The mint's token program; read from the mint account when omitted. */
    tokenProgram?: Address;
  },
): Promise<ResolvedPlatformFee | null> {
  const { fee, mint } = input;
  if (!fee || fee.bps <= 0) return null;

  let tokenProgram = input.tokenProgram;
  if (tokenProgram === undefined) {
    const mintAccount = await fetchEncodedAccount(rpc, mint);
    if (!mintAccount.exists) return null;
    tokenProgram = mintAccount.programAddress;
  }

  const [feeAccount] = await findAssociatedTokenPda({
    owner: fee.wallet,
    mint,
    tokenProgram,
  });
  const account = await fetchEncodedAccount(rpc, feeAccount);
  return account.exists ? { bps: fee.bps, feeAccount } : null;
}

export type DflowQuoteInput = {
  /** Full `/quote` endpoint URL, e.g. `https://quote-api.dflow.net/quote`. */
  quoteUrl: string;
  /**
   * Mints as the **aggregator** knows them — the canonical (mainnet) mints.
   * These are not necessarily the mints the eCLOB leg routes against; a
   * localnet deployment quotes its own mock mints on-chain while DFlow can
   * only price the real ones.
   */
  inputMint: string;
  outputMint: string;
  /** Input amount in atomic units. */
  amount: bigint;
  /** `'auto'` lets DFlow size slippage from live liquidity; else basis points. */
  slippageBps: 'auto' | number;
  /** Declared only when non-null — see {@link resolvePlatformFee}. */
  platformFee?: ResolvedPlatformFee | null;
  /** Optional production API key, sent as `x-api-key`. */
  apiKey?: string;
  signal?: AbortSignal;
};

/** A validated DFlow quote. `outAmount` is net of any declared platform fee. */
export type DflowQuote = {
  inAmount: bigint;
  outAmount: bigint;
  priceImpactPct: string | null;
  slippageBps: number | null;
  /** Echoed back by DFlow when a fee was declared and applied. */
  platformFee: { amount: bigint; feeBps: number; feeAccount: string } | null;
};

/** Validate a `/quote` body into a {@link DflowQuote}. */
export function parseDflowQuote(raw: unknown): DflowQuote {
  if (!isObject(raw)) {
    throw new DflowError('quote response is not an object', 'api');
  }
  const rawFee = raw.platformFee;
  return {
    inAmount: parseAmount(raw.inAmount, 'quote.inAmount'),
    outAmount: parseAmount(raw.outAmount, 'quote.outAmount'),
    priceImpactPct: isString(raw.priceImpactPct) ? raw.priceImpactPct : null,
    slippageBps: isNumber(raw.slippageBps) ? raw.slippageBps : null,
    platformFee: isObject(rawFee)
      ? {
          amount: parseAmount(rawFee.amount, 'quote.platformFee.amount'),
          feeBps: isNumber(rawFee.feeBps) ? rawFee.feeBps : 0,
          feeAccount: isString(rawFee.feeAccount) ? rawFee.feeAccount : '',
        }
      : null,
  };
}

/**
 * Fetch and validate a DFlow `/quote`.
 *
 * When `platformFee` is supplied the fee is declared on the request, so the
 * returned `outAmount` is **net of it** — which is what makes an aggregator
 * quote comparable to an eCLOB simulation (whose `outAmount` is likewise net
 * of the on-chain taker fee). `platformFeeMode` is pinned to `outputMint`
 * rather than left to the server default, since the resolved `feeAccount` is
 * an ATA of the output mint and the two must agree.
 *
 * Throws {@link DflowError} — `rateLimited` on 429, `api` on any other non-2xx
 * or an unparseable body, `network` when the request never completed.
 */
export async function fetchDflowQuote(
  input: DflowQuoteInput,
): Promise<DflowQuote> {
  const url = new URL(input.quoteUrl);
  url.searchParams.set('inputMint', input.inputMint);
  url.searchParams.set('outputMint', input.outputMint);
  url.searchParams.set('amount', input.amount.toString());
  url.searchParams.set('slippageBps', String(input.slippageBps));
  if (input.platformFee) {
    url.searchParams.set('platformFeeBps', String(input.platformFee.bps));
    url.searchParams.set('feeAccount', input.platformFee.feeAccount);
    url.searchParams.set('platformFeeMode', 'outputMint');
  }

  const headers: Record<string, string> = {};
  if (input.apiKey) headers['x-api-key'] = input.apiKey;

  let res: Response;
  try {
    res = await fetch(url.toString(), { headers, signal: input.signal });
  } catch (e) {
    // Preserve an abort so callers can distinguish "we cancelled this" from a
    // genuine transport failure; everything else is a network error.
    if (e instanceof DOMException && e.name === 'AbortError') throw e;
    const detail = e instanceof Error ? e.message : String(e);
    throw new DflowError(`Network error reaching DFlow: ${detail}`, 'network');
  }

  if (res.status === 429) {
    throw new DflowError('DFlow rate limit reached', 'rateLimited', 429);
  }
  if (!res.ok) {
    const info = await extractDflowApiError(res);
    throw new DflowError(info.message, 'api', res.status, info.code ?? undefined);
  }

  let body: unknown;
  try {
    body = await res.json();
  } catch {
    throw new DflowError('DFlow quote body was not JSON', 'api', res.status);
  }
  return parseDflowQuote(body);
}
