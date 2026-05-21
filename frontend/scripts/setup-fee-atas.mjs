// One-time admin script: create the platform-fee ATAs that DFlow's /order
// endpoint requires. DFlow refuses to route the fee unless the destination
// token account already exists onchain, so we pre-create one per currency
// in currencies.json. Idempotent — safe to re-run after adding new mints.
//
// Env:
//   FEE_WALLET_KEYPAIR  Path to the fee wallet's secret key file. Two formats
//                       are accepted (detected by content, not extension):
//                         • Solana CLI JSON: a 64-element JSON byte array.
//                         • Phantom export:  a base58-encoded 64-byte secret
//                                            (the string "Show Private Key"
//                                            copies to the clipboard), saved
//                                            as plain text.
//                       Defaults to ~/.config/solana/id.json.
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
  getCreateAssociatedTokenIdempotentInstructionAsync,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";

const here = dirname(fileURLToPath(import.meta.url));
const currencies = JSON.parse(
  readFileSync(resolve(here, "../lib/currencies.json"), "utf8"),
);

const KEYPAIR_PATH = (
  process.env.FEE_WALLET_KEYPAIR ?? "~/.config/solana/id.json"
).replace(/^~(?=$|\/)/, homedir());
const RPC_URL =
  process.env.RPC_URL ??
  process.env.NEXT_PUBLIC_RPC_URL ??
  "https://api.mainnet-beta.solana.com";
const DRY_RUN = process.env.DRY_RUN === "1";

// Conservative cap. ATA-create takes ~4 unique accounts after dedup; 8 ix per
// tx keeps us well clear of the ~64-account static-key limit and the 1232-byte
// tx-size ceiling, with no need for an Address Lookup Table.
const ATAS_PER_TX = 8;
const CONFIRM_TIMEOUT_MS = 60_000;
const POLL_INTERVAL_MS = 1000;

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

const signer = await createKeyPairSignerFromBytes(
  loadKeypairBytes(KEYPAIR_PATH),
);
const rpc = createSolanaRpc(RPC_URL);

console.log(`Fee wallet:     ${signer.address}`);
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
    const ix = await getCreateAssociatedTokenIdempotentInstructionAsync({
      payer: signer,
      owner: signer.address,
      mint: address(t.mint),
      tokenProgram: programAddress,
    });
    // The ATA account is the second positional account in the ix.
    const ata = ix.accounts[1].address;
    return { symbol: t.symbol, mint: t.mint, ata, ix };
  }),
);

const accountInfos = await rpc
  .getMultipleAccounts(
    plan.map((p) => p.ata),
    { commitment: "confirmed" },
  )
  .send();

const missing = plan.filter((_, i) => accountInfos.value[i] === null);
const existing = plan.length - missing.length;

console.log(`Already exist:  ${existing}`);
console.log(`To create:      ${missing.length}`);
if (missing.length === 0) {
  console.log("Nothing to do.");
  process.exit(0);
}

for (const m of missing) {
  console.log(`  + ${m.symbol.padEnd(6)} ${m.ata}`);
}

if (DRY_RUN) {
  console.log("\nDRY_RUN=1 — not sending transactions.");
  process.exit(0);
}

const ok = await confirm(
  `\nCreate ${missing.length} ATA(s) owned by ${signer.address}? [y/N] `,
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
        batch.map((x) => x.ix),
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
