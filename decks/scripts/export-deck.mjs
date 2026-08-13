import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { decks } from "../lib/decks.mjs";
import { captureDeck } from "./capture.mjs";
import { buildPptx } from "./pptx.mjs";

/**
 * Export a deck to `.pptx` from the command line.
 *
 * Usage:
 *
 *   pnpm run export                 # the first deck in the registry
 *   pnpm run export -- /demo-v1     # a specific deck route
 *
 * This is the only way to build a `.pptx`. The site used to offer the same
 * thing as a download, backed by a `GET /api/export` route this command was a
 * thin client of — but an export is a headless browser run, which is a poor
 * fit for a serverless function and broke outright for a visitor on a machine
 * the deployment had never been exercised from. Nobody needs a deck built from
 * the deployed site: whoever wants one has the checkout. So the route is gone
 * and the pipeline lives here.
 *
 * A server is still needed, because a capture is screenshots of real pages —
 * so this finds one or starts one, then drives the same `capture.mjs` +
 * `pptx.mjs` pair the route drove.
 */

const PORT = Number(process.env.DECK_EXPORT_PORT ?? 3310);
const ORIGIN = `http://localhost:${PORT}`;
const OUT_DIR = "out";

/** How long to wait for a freshly-spawned dev server to answer. */
const SERVER_TIMEOUT_MS = 120000;
const POLL_INTERVAL_MS = 500;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** `Colosseum Cohort 5 Demo Day` → `colosseum-cohort-5-demo-day`. */
const slugify = (title) =>
  title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");

/** True once the server answers at all — any status means it is listening. */
async function isUp(origin) {
  try {
    await fetch(origin, { signal: AbortSignal.timeout(2000) });
    return true;
  } catch {
    return false;
  }
}

/**
 * Start `next dev` on the export port and resolve once it responds.
 *
 * A dedicated port rather than the 3300 `pnpm dev` uses, so exporting never
 * collides with a dev server someone is already working in.
 */
async function startServer() {
  // Spawned by its path in the package's own `node_modules/.bin` rather than
  // by name: this runs as a pnpm script from the package root, so the binary
  // is right there, and relying on PATH would make the command depend on how
  // it was invoked.
  const server = spawn(join("node_modules", ".bin", "next"), ["dev", "--port", String(PORT)], {
    stdio: "ignore",
  });

  const deadline = Date.now() + SERVER_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) {
      throw new Error(`dev server exited early with code ${server.exitCode}`);
    }
    if (await isUp(ORIGIN)) return server;
    await sleep(POLL_INTERVAL_MS);
  }

  server.kill();
  throw new Error(`dev server did not come up within ${SERVER_TIMEOUT_MS}ms`);
}

async function main() {
  const route = process.argv[2] ?? decks[0]?.route;
  const deck = decks.find((candidate) => candidate.route === route);

  if (!deck) {
    const available = decks.map((candidate) => candidate.route).join(", ");
    throw new Error(`unknown deck: ${route}\nAvailable: ${available}`);
  }

  let server = null;
  if (await isUp(ORIGIN)) {
    console.log(`Using the server already listening on ${ORIGIN}`);
  } else {
    console.log(`Starting a deck server on ${ORIGIN} …`);
    server = await startServer();
  }

  try {
    console.log(`Capturing ${deck.title} — this takes a few seconds per page …`);
    const shots = await captureDeck({
      baseUrl: ORIGIN,
      route: deck.route,
      pages: deck.pages,
      onPage: (done, total) => console.log(`  page ${done}/${total}`),
    });

    const pptx = await buildPptx(shots);

    await mkdir(OUT_DIR, { recursive: true });
    const target = join(OUT_DIR, `${slugify(deck.title)}.pptx`);
    await writeFile(target, pptx);

    console.log(`\nWrote ${target}`);
    console.log(
      "Import it with Google Slides ▸ File ▸ Import slides, or open it in " +
        "PowerPoint or Keynote.",
    );
  } finally {
    server?.kill();
  }
}

main().catch((error) => {
  console.error(`\n${error.message}`);
  process.exit(1);
});
