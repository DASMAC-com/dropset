// Admin script for the platform-fee ATAs that DFlow's /order endpoint
// requires. DFlow refuses to route the fee unless the destination token
// account already exists onchain, so we keep one per currency in
// currencies.json. Idempotent — safe to re-run after adding new mints.
//
// Two modes:
//   (default)  Create whatever is missing. Needs FEE_WALLET_KEYPAIR.
//   --check    Read-only audit. Exits non-zero if any roster mint has no fee
//              vault. Needs no key at all, which is what lets CI run it: the
//              check only derives addresses and reads accounts. Creating a
//              vault needs the secret key; verifying one needs nothing
//              private.
//
// Both modes derive addresses through the SAME findAssociatedTokenPda call
// below. That is deliberate — a checker with its own derivation could report
// "all present" against addresses the creator never writes to, which is
// exactly the silent-revenue-leak failure this script exists to prevent.
//
// Env:
//   FEE_WALLET_KEYPAIR  Path to the fee wallet's secret key file. Two formats
//                       are accepted (detected by content, not extension):
//                         • Solana CLI JSON: a 64-element JSON byte array.
//                         • Phantom export:  a base58-encoded 64-byte secret
//                                            (the string "Show Private Key"
//                                            copies to the clipboard), saved
//                                            as plain text.
//                       Defaults to ~/.config/solana/id.json. Unused by
//                       --check.
//   NEXT_PUBLIC_PLATFORM_FEE_WALLET
//                       The fee wallet's public address. Required in both
//                       modes; never committed. Locally it comes from
//                       frontend/.env.local, in CI from a repository secret.
//   RPC_URL             RPC endpoint. Falls back to NEXT_PUBLIC_RPC_URL,
//                       then mainnet-beta. Public RPC will likely throttle;
//                       point this at your provider for reliable runs.
//   DRY_RUN=1           Print what would be created, don't send any tx.
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, resolve } from "node:path";
import { createInterface } from "node:readline/promises";
import { fileURLToPath } from "node:url";

import {
  address,
  appendTransactionMessageInstructions,
  createKeyPairSignerFromBytes,
  createSolanaRpc,
  createTransactionMessage,
  getBase58Encoder,
  getBase64EncodedWireTransaction,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signTransactionMessageWithSigners,
} from "@solana/kit";
import {
  findAssociatedTokenPda,
  getCreateAssociatedTokenIdempotentInstruction,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";

const here = dirname(fileURLToPath(import.meta.url));
const currencies = JSON.parse(
  readFileSync(resolve(here, "../lib/data/currencies.json"), "utf8"),
);

// Pick up frontend/.env.local, the same file `next dev` reads, so a local run
// need not restate the fee wallet on the command line. Best-effort: the file
// is absent in CI, where the address arrives as a repository secret. Node
// leaves already-set variables alone, so a value passed explicitly on the
// command line still wins over a stale file — which matters here, because a
// silently overridden owner would send real transactions to the wrong wallet.
try {
  process.loadEnvFile(resolve(here, "../.env.local"));
} catch {
  // No .env.local — fall through to the real environment.
}

const CHECK_ONLY = process.argv.includes("--check");

// The fee wallet's public address. Deliberately NOT committed — it is
// supplied by the environment everywhere, including CI, so the repository
// never carries the address of the wallet that collects platform fees. In
// GitHub Actions it arrives from a secret rather than a variable, because
// this script echoes the address and a variable would land it in the job log
// in plaintext.
const feeWalletRaw = process.env.NEXT_PUBLIC_PLATFORM_FEE_WALLET?.trim();
if (!feeWalletRaw) {
  throw new Error(
    "NEXT_PUBLIC_PLATFORM_FEE_WALLET is unset. It holds the fee wallet's " +
      "public address and is required in both modes. Locally it comes from " +
      "frontend/.env.local; in CI it comes from the repository secret of " +
      "the same name.",
  );
}
const EXPECTED_FEE_WALLET = address(feeWalletRaw);

const KEYPAIR_PATH = (
  process.env.FEE_WALLET_KEYPAIR ?? "~/.config/solana/id.json"
).replace(/^~(?=$|\/)/, homedir());
const RPC_URL =
  process.env.RPC_URL ??
  process.env.NEXT_PUBLIC_RPC_URL ??
  "https://api.mainnet-beta.solana.com";
const DRY_RUN = process.env.DRY_RUN === "1";

// Conservative cap. ATA-create takes ~4 unique accounts after de-dupe; 8 ix per
// tx keeps us well clear of the ~64-account static-key limit and the 1232-byte
// tx-size ceiling, with no need for an Address Lookup Table.
const ATAS_PER_TX = 8;
// getMultipleAccounts caps at 100 addresses per call. The roster is far under
// that today, but chunking costs nothing and removes a cliff that would
// otherwise appear as an opaque RPC error the first time it is crossed.
const ACCOUNTS_PER_QUERY = 100;
const CONFIRM_TIMEOUT_MS = 60_000;
const POLL_INTERVAL_MS = 1000;
// The --check mode gates the merge queue, so a throttled public RPC must not
// red-line unrelated merges. Only a read that fails every attempt fails the
// job — the same reasoning the icon-URL gates in frontend.yml already use.
const RPC_ATTEMPTS = 3;
const RPC_RETRY_BASE_MS = 2000;

const PROGRAM_FOR_KIND = {
  classic: TOKEN_PROGRAM_ADDRESS,
  token2022: TOKEN_2022_PROGRAM_ADDRESS,
};

// Two accepted shapes. JSON-array branch matches the Solana CLI format;
// the base58 branch matches what Phantom's "Show Private Key" copies. We
// dispatch by leading character so callers don't have to pick a flag.
function loadKeypairBytes(path) {
  const raw = readFileSync(path, "utf8").trim();
  if (raw.startsWith("[")) {
    const arr = JSON.parse(raw);
    if (!Array.isArray(arr) || arr.length !== 64) {
      throw new Error(
        `Keypair file ${path} is not a 64-byte JSON array (Solana CLI format).`,
      );
    }
    return Uint8Array.from(arr);
  }
  const bytes = getBase58Encoder().encode(raw);
  if (bytes.length !== 64) {
    throw new Error(
      `Keypair file ${path} decoded to ${bytes.length} bytes (need 64). Phantom exports the full secret key; 32-byte seed-only is not supported.`,
    );
  }
  return bytes;
}

async function confirm(question) {
  if (!process.stdin.isTTY) {
    throw new Error(
      "Cannot prompt for confirmation — stdin is not a TTY. Re-run interactively or set DRY_RUN=1.",
    );
  }
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    const answer = (await rl.question(question)).trim();
    return /^y(es)?$/i.test(answer);
  } finally {
    rl.close();
  }
}

function chunk(arr, n) {
  const out = [];
  for (let i = 0; i < arr.length; i += n) out.push(arr.slice(i, i + n));
  return out;
}

async function withRetry(label, fn) {
  let lastError;
  for (let attempt = 1; attempt <= RPC_ATTEMPTS; attempt++) {
    try {
      return await fn();
    } catch (err) {
      lastError = err;
      if (attempt === RPC_ATTEMPTS) break;
      console.warn(
        `  ${label} failed (attempt ${attempt}/${RPC_ATTEMPTS}), retrying...`,
      );
      await new Promise((r) => setTimeout(r, RPC_RETRY_BASE_MS * attempt));
    }
  }
  throw lastError;
}

async function waitForConfirmation(rpc, signature) {
  const deadline = Date.now() + CONFIRM_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const { value } = await rpc.getSignatureStatuses([signature]).send();
    const status = value[0];
    if (status?.err) {
      throw new Error(
        `Transaction reverted: ${JSON.stringify(status.err)} (sig ${signature})`,
      );
    }
    const cs = status?.confirmationStatus;
    if (cs === "confirmed" || cs === "finalized") return;
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }
  throw new Error(`Timed out waiting for confirmation of ${signature}`);
}

const tokens = Object.values(currencies).flatMap((entry) => entry.stablecoins);

// The create path loads the key; --check deliberately does not, so CI never
// needs one. Guarding the signer against EXPECTED_FEE_WALLET is the single
// most important safety property here: a freshly generated keypair would
// happily create valid ATAs owned by an address the frontend never
// references, so fees would accrue where nothing points, the existence check
// would still fail, and the fee would keep not being charged — a failure
// that looks exactly like success.
let signer = null;
if (!CHECK_ONLY) {
  signer = await createKeyPairSignerFromBytes(loadKeypairBytes(KEYPAIR_PATH));
  if (signer.address !== EXPECTED_FEE_WALLET) {
    throw new Error(
      `Keypair ${KEYPAIR_PATH} is for ${signer.address}, but the configured ` +
        `platform fee wallet is ${EXPECTED_FEE_WALLET}. Vaults created under ` +
        `the wrong owner are invisible to the frontend and silently forfeit ` +
        `the fee. Point FEE_WALLET_KEYPAIR at the real fee wallet, or set ` +
        `NEXT_PUBLIC_PLATFORM_FEE_WALLET if the wallet has been rotated.`,
    );
  }
}
const owner = signer ? signer.address : EXPECTED_FEE_WALLET;
const rpc = createSolanaRpc(RPC_URL);

console.log(`Mode:           ${CHECK_ONLY ? "check (read-only)" : "create"}`);
console.log(`Fee wallet:     ${owner}`);
console.log(`RPC:            ${RPC_URL}`);
console.log(`Currencies:     ${tokens.length}`);

// Build the {mint, tokenProgram} → desired ATA plan, then query the chain to
// find which ATAs are missing. Idempotent instructions would handle the
// already-exists case, but skipping ahead saves an unnecessary tx + fee when
// nothing has changed.
const plan = await Promise.all(
  tokens.map(async (t) => {
    const programAddress = PROGRAM_FOR_KIND[t.tokenProgram];
    if (!programAddress) {
      throw new Error(
        `Unknown tokenProgram "${t.tokenProgram}" for ${t.symbol}`,
      );
    }
    const [ata] = await findAssociatedTokenPda({
      owner,
      tokenProgram: programAddress,
      mint: address(t.mint),
    });
    return { symbol: t.symbol, mint: t.mint, ata, programAddress };
  }),
);

// A transport failure here is an inconclusive run, not a missing vault, and
// the two must never look alike: reporting "vault missing" because an RPC
// refused us would send someone to spend rent on an account that already
// exists. Exit 2 to keep it distinguishable from the exit 1 that means a
// vault is genuinely absent, and report it without a stack — this runs as a
// merge gate, where the actionable line is which endpoint failed.
const accountInfos = [];
try {
  for (const batch of chunk(plan, ACCOUNTS_PER_QUERY)) {
    const { value } = await withRetry("getMultipleAccounts", () =>
      rpc
        .getMultipleAccounts(
          batch.map((p) => p.ata),
          { commitment: "confirmed" },
        )
        .send(),
    );
    accountInfos.push(...value);
  }
} catch (err) {
  console.error(
    `\nERROR: could not read account state from ${RPC_URL} after ` +
      `${RPC_ATTEMPTS} attempts: ${err.message}\n` +
      `This is inconclusive, not a verdict — no vault has been shown ` +
      `missing. Set RPC_URL to an endpoint that serves getMultipleAccounts ` +
      `and re-run.`,
  );
  process.exit(2);
}

const missing = plan.filter((_, i) => accountInfos[i] === null);
const existing = plan.length - missing.length;

console.log(`Already exist:  ${existing}`);
console.log(`To create:      ${missing.length}`);

for (const m of missing) {
  console.log(`  + ${m.symbol.padEnd(6)} ${m.ata}`);
}

// --check is a gate, so a missing vault is an error rather than a to-do list.
// Every listed currency is assumed fee-eligible: there is no opt-out, by
// operator direction, so listing a currency and funding its vault stay
// coupled and a gap can never sit unnoticed the way EUROP's did.
if (CHECK_ONLY) {
  if (missing.length > 0) {
    console.error(
      `\nFAIL: ${missing.length} listed currency/currencies have no platform-fee ` +
        `vault under ${owner}. DFlow rejects any /order whose feeAccount does ` +
        `not exist, so the platform fee is silently forfeited on every ` +
        `DFlow-routed swap into these mints.\n\n` +
        `Fix: create them, then re-run this check.\n` +
        `  FEE_WALLET_KEYPAIR=<path> pnpm --dir frontend setup-fee-atas`,
    );
    process.exit(1);
  }
  console.log("\nOK: every listed currency has a platform-fee vault.");
  process.exit(0);
}

if (missing.length === 0) {
  console.log("Nothing to do.");
  process.exit(0);
}

if (DRY_RUN) {
  console.log("\nDRY_RUN=1 — not sending transactions.");
  process.exit(0);
}

const ok = await confirm(
  `\nCreate ${missing.length} ATA(s) owned by ${owner}? [y/N] `,
);
if (!ok) {
  console.log("Aborted.");
  process.exit(1);
}

const batches = chunk(missing, ATAS_PER_TX);
console.log(`\nSending ${batches.length} transaction(s)...`);

for (let b = 0; b < batches.length; b++) {
  const batch = batches[b];
  const { value: blockhash } = await rpc
    .getLatestBlockhash({ commitment: "confirmed" })
    .send();

  const message = pipe(
    createTransactionMessage({ version: 0 }),
    (m) => setTransactionMessageFeePayerSigner(signer, m),
    (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
    (m) =>
      appendTransactionMessageInstructions(
        batch.map((x) =>
          getCreateAssociatedTokenIdempotentInstruction({
            payer: signer,
            ata: x.ata,
            owner,
            mint: address(x.mint),
            tokenProgram: x.programAddress,
          }),
        ),
        m,
      ),
  );
  const signed = await signTransactionMessageWithSigners(message);
  const encoded = getBase64EncodedWireTransaction(signed);

  const signature = await rpc
    .sendTransaction(encoded, {
      encoding: "base64",
      preflightCommitment: "confirmed",
    })
    .send();

  const symbols = batch.map((x) => x.symbol).join(", ");
  console.log(`  [${b + 1}/${batches.length}] ${signature}  (${symbols})`);
  await waitForConfirmation(rpc, signature);
}

console.log(`\nDone. Created ${missing.length} fee ATA(s).`);
