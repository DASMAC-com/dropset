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
//
// ONLY the fetch happens in here, and that matters. `committedIconFor` used to
// be called in this callback too, and because it is the one helper that THROWS
// (a symbol with two committed icons), its rejection was indistinguishable from
// a fetch rejection: a purely local, deterministic repo problem got reported as
// `declared upstream source unreachable`, pointing the reader at the issuer's
// host instead of at the stale file in their own checkout — and on --write it
// counted toward "N fetch(es) failed" for a token no fetch was attempted for.
// Resolving the committed file in the loop below keeps local and network
// failures in separate buckets.
const results = await Promise.allSettled(
  tokens.map((token) => fetchWithRetry(token.icon)),
);

const matched = [];
const wrote = [];
const drifted = [];
const uncommitted = [];
const conflicts = [];
const unreachable = [];

for (let i = 0; i < results.length; i++) {
  const result = results[i];
  const token = tokens[i];
  const label = `${token.symbol} (${token.mint})`;

  if (result.status === "rejected") {
    // Collapse newlines: this string reaches stdout behind a `::warning::`
    // prefix, and GitHub parses a workflow command only at the start of a line.
    // The message is built from our own templates, one of which interpolates a
    // response header, so a line break here is the only thing that could forge
    // a second annotation. Cheaper to remove than to rely on the HTTP client
    // rejecting it.
    // `?? result.reason` because a rejection value need not be an object at
    // all; reading `.message` off a primitive would TypeError from OUTSIDE the
    // allSettled region and take the process down, which is the same failure
    // class the isFile() filter closes and would break the same exit-0
    // guarantee. fetchWithRetry does always reject with an Error today, so this
    // is insurance rather than a live bug.
    unreachable.push(
      `${label}: ${String(result.reason?.message ?? result.reason).replace(/[\r\n]+/g, " ")}`,
    );
    continue;
  }

  const { ext, buf } = result.value;

  let committed;
  try {
    committed = committedIconFor(token.symbol, entries);
  } catch (err) {
    conflicts.push(`${label}: ${err.message}`);
    continue;
  }

  if (write) {
    // Write under the extension upstream's bytes actually sniff as, so a
    // format change upstream lands as a new filename rather than a `.png`
    // holding an SVG. The old file is left in place on purpose, so the
    // duplicate is noticed rather than silently shadowed — it surfaces as a
    // `token icon committed twice` warning on the NEXT audit run (`entries` is
    // snapshotted before the loop, so a duplicate this run creates cannot warn
    // this run), and as a reported conflict rather than a crash from
    // build-token-manifest.mjs. Deleting the stale file is the reviewer's job
    // when they commit the refresh.
    const filename = `${token.symbol}.${ext}`;
    writeFileSync(join(ICON_DIR, filename), buf);
    wrote.push(`${label}: wrote ${filename} (${buf.length}B)`);
    continue;
  }

  if (!committed) {
    uncommitted.push(
      `${label}: nothing committed for this symbol (upstream serves ${buf.length}B of ${ext})`,
    );
    continue;
  }

  // Read once. Comparing one read and reporting the length of a second made the
  // reported size not provably the size of what was compared.
  const committedBytes = readFileSync(committed.path);
  if (committedBytes.equals(buf)) {
    matched.push(`${label}: ${committed.filename}`);
  } else {
    drifted.push(
      `${label}: ${committed.filename} differs from ${token.icon} (committed ${committedBytes.length}B, upstream ${buf.length}B)`,
    );
  }
}

// GitHub renders `::warning::` as an annotation on the run, which is the
// "warning" half of the specified behavior. The matched (or written) list goes
// out on a plain console.log, so a green run still shows what was checked.
const warn = (message) => console.log(`::warning::${message}`);

console.log(
  write
    ? `Wrote ${wrote.length}/${tokens.length} token icon(s) into brand-assets/token-icons.`
    : `Byte-identical: ${matched.length}/${tokens.length} committed token icon(s) match their declared upstream source.`,
);
for (const line of write ? wrote : matched) console.log(`  - ${line}`);

for (const line of drifted) warn(`token icon drift — ${line}`);
for (const line of uncommitted) warn(`token icon not committed — ${line}`);
for (const line of conflicts) warn(`token icon committed twice — ${line}`);
for (const line of unreachable)
  warn(`declared upstream source unreachable — ${line}`);

if (
  !write &&
  (drifted.length ||
    uncommitted.length ||
    conflicts.length ||
    unreachable.length)
) {
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
      "  - committed twice → a past --write left the pre-format-change file behind; delete the stale one",
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
// to produce bytes, so anything it declined to produce must be visible as a
// failure. That includes a CONFLICT, not just a failed fetch — a symbol
// skipped because it already has two committed icons is a partial refresh
// exactly as much as an unreachable host is, and gating only on `unreachable`
// let such a symbol be skipped under a cheerful `Wrote 24/25` and exit 0.
if (write && (unreachable.length || conflicts.length)) {
  const reasons = [];
  if (unreachable.length) {
    reasons.push(`${unreachable.length} fetch(es) failed`);
  }
  if (conflicts.length) {
    reasons.push(
      `${conflicts.length} symbol(s) already have two committed icons`,
    );
  }
  console.error(
    `\n--write: ${reasons.join(", ")} out of ${tokens.length}; those icons were NOT refreshed.`,
  );
  process.exitCode = 1;
}
