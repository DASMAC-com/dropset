import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

/**
 * Export a deck to `.pptx` from the command line.
 *
 * Usage:
 *
 *   pnpm run export                 # the first deck in the registry
 *   pnpm run export -- /demo-v1     # a specific deck route
 *
 * This is deliberately a thin client of `GET /api/export`: the capture and
 * `.pptx` assembly live in the route so that the command and the download
 * button on the site cannot produce different files. All this adds is finding
 * or starting a server, and writing the response to `out/`.
 */

const PORT = Number(process.env.DECK_EXPORT_PORT ?? 3310);
const ORIGIN = `http://localhost:${PORT}`;
const OUT_DIR = "out";

/** How long to wait for a freshly-spawned dev server to answer. */
const SERVER_TIMEOUT_MS = 120000;
const POLL_INTERVAL_MS = 500;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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
  const route = process.argv[2];

  let server = null;
  if (await isUp(ORIGIN)) {
    console.log(`Using the server already listening on ${ORIGIN}`);
  } else {
    console.log(`Starting a deck server on ${ORIGIN} …`);
    server = await startServer();
  }

  try {
    const url = new URL("/api/export", ORIGIN);
    if (route) url.searchParams.set("deck", route);

    console.log("Capturing pages — this takes a few seconds per page …");
    const response = await fetch(url);

    if (!response.ok) {
      const body = await response.text();
      throw new Error(`export failed (${response.status}): ${body}`);
    }

    // The route names the file; mirroring it keeps the CLI's output and the
    // browser download identical.
    const disposition = response.headers.get("content-disposition") ?? "";
    const name = /filename="(.+?)"/.exec(disposition)?.[1] ?? "deck.pptx";

    await mkdir(OUT_DIR, { recursive: true });
    const target = join(OUT_DIR, name);
    await writeFile(target, Buffer.from(await response.arrayBuffer()));

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
