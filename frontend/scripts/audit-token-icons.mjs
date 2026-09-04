// Audit the committed token icons against the upstream sources that
// currencies.json declares for them. This is the *only* place the token-icon
// pipeline touches the network, and it is deliberately not on the build path:
// the build reads committed bytes (build-token-manifest.mjs) and cannot fail
// on a third-party host.
//
// Two modes:
//
//   (default)  fetch each declared URL and report whether the committed
//              bytes are byte-identical to what upstream serves today.
//              ALWAYS exits 0 — see the exit-code note at the bottom.
//   --write    fetch and write the bytes into brand-assets/token-icons/.
//              This is how the directory was seeded and how an icon is
//              refreshed after a deliberate upstream change. Exits non-zero
//              if any fetch failed, because a partial refresh is a real
//              failure of the thing you asked for.
//
// Byte-identity is only meaningful because nothing in the pipeline re-encodes
// or optimizes: `fetchWithRetry` hands back the raw response body and
// --write stores exactly that. Any image optimization introduced anywhere
// between upstream and the committed file would make this audit
// false-positive forever.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fetchWithRetry } from "./mirror-icons.mjs";
import {
  committedIconFor,
  ICON_DIR,
  listCommitted,
  readTokens,
} from "./token-icons-shared.mjs";

const write = process.argv.includes("--write");
const tokens = readTokens();

if (write) mkdirSync(ICON_DIR, { recursive: true });
const entries = listCommitted();

// Fetch every declared URL concurrently. `allSettled` because one dead host
// must not hide the verdict for the other 24 tokens — the whole point of the
// audit is the full picture, and a `Promise.all` would report the first
// rejection and abandon the rest.
const results = await Promise.allSettled(
  tokens.map(async (token) => {
    const committed = committedIconFor(token.symbol, entries);
    const { ext, buf } = await fetchWithRetry(token.icon);
    return { token, committed, ext, buf };
  }),
);

const identical = [];
const drifted = [];
const uncommitted = [];
const unreachable = [];

for (let i = 0; i < results.length; i++) {
  const result = results[i];
  const token = tokens[i];
  const label = `${token.symbol} (${token.mint})`;

  if (result.status === "rejected") {
    unreachable.push(`${label}: ${result.reason.message}`);
    continue;
  }

  const { committed, ext, buf } = result.value;

  if (write) {
    // Write under the extension upstream's bytes actually sniff as, so a
    // format change upstream lands as a new filename rather than a `.png`
    // holding an SVG. The old file is left in place on purpose: the
    // duplicate then trips committedIconFor's two-icons error, which is a
    // louder and more accurate signal than silently shadowing it.
    const filename = `${token.symbol}.${ext}`;
    writeFileSync(join(ICON_DIR, filename), buf);
    identical.push(`${label}: wrote ${filename} (${buf.length}B)`);
    continue;
  }

  if (!committed) {
    uncommitted.push(
      `${label}: nothing committed for this symbol (upstream serves ${buf.length}B of ${ext})`,
    );
    continue;
  }

  if (readFileSync(committed.path).equals(buf)) {
    identical.push(`${label}: ${committed.filename}`);
  } else {
    drifted.push(
      `${label}: ${committed.filename} differs from ${token.icon} (committed ${readFileSync(committed.path).length}B, upstream ${buf.length}B)`,
    );
  }
}

// GitHub renders `::warning::` as an annotation on the run, which is the
// "warning" half of the specified behavior. Plain console.log for the
// identical set so a green run still shows what was checked.
const warn = (message) => console.log(`::warning::${message}`);

if (write) {
  console.log(
    `Wrote ${identical.length}/${tokens.length} token icon(s) into brand-assets/token-icons.`,
  );
  for (const line of identical) console.log(`  - ${line}`);
} else {
  console.log(
    `Byte-identical: ${identical.length}/${tokens.length} committed token icon(s) match their declared upstream source.`,
  );
  for (const line of identical) console.log(`  - ${line}`);
}

for (const line of drifted) warn(`token icon drift — ${line}`);
for (const line of uncommitted) warn(`token icon not committed — ${line}`);
for (const line of unreachable)
  warn(`declared upstream source unreachable — ${line}`);

if (!write && (drifted.length || uncommitted.length || unreachable.length)) {
  // The follow-up task the redesign calls for is communicated by this job
  // rather than filed by it: CI holds no Linear credential, and a
  // GitHub-issue filer was declined in favor of the job saying its piece.
  // Whoever reads the annotation decides whether the drift is an intentional
  // upstream change (refresh with --write) or link rot (point the URL
  // somewhere else).
  console.log(
    [
      "",
      "Follow-up needed. For each token above:",
      "  - upstream changed deliberately → `pnpm --filter dropset-frontend audit-token-icons --write`, review the diff, commit it",
      "  - the URL is dead or now serves the wrong artwork → update `icon` in frontend/lib/data/currencies.json",
      "  - the host was merely down → re-run this job; nothing is blocked meanwhile",
    ].join("\n"),
  );
}

// Exit 0 no matter what the audit found. This is the property the whole
// redesign exists to establish: a third-party host cannot turn red here.
// The job is also not in the required-check set, so even a hard crash
// could not block the merge queue — the exit code is the second layer, not
// the only one.
//
// --write is different: it is a local refresh command whose entire job is
// to produce bytes, so a failed fetch there must be visible as a failure.
if (write && unreachable.length) {
  console.error(
    `\n--write: ${unreachable.length}/${tokens.length} fetch(es) failed; those icons were NOT refreshed.`,
  );
  process.exitCode = 1;
}
