import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

/**
 * Screenshot a deck's pages with a headless Chromium.
 *
 * Why screenshots rather than Spectacle's own export mode plus print-to-PDF —
 * the route `decks/README.md` used to describe: that path renders through
 * Spectacle's export theme, which supplies its own white `Backdrop` and an
 * empty `backdropStyle`, and on a dark deck the result is inverted artwork and
 * a vanished wordmark (an opaque PNG shown with `mix-blend-mode: screen`, and
 * screening over white returns white). It also dead-ends: Google Slides cannot
 * import a PDF, so the pages would still need rasterizing with a tool the repo
 * does not carry.
 *
 * Screenshotting the live page instead takes its pixels from the same renderer
 * the deck is reviewed in, so what exports is by construction what was
 * approved, and it produces the PNGs the `.pptx` needs directly.
 */

/**
 * The deck's design space, and therefore the capture viewport. Kept in step
 * with `DECK_SIZE` in `theme/tokens.ts` — capturing at the size the deck is
 * laid out for means no scaling, so text is rendered rather than resampled.
 */
export const CAPTURE_SIZE = { width: 1920, height: 1080 };

/**
 * Chromium builds that ship a compatible headless mode, most-preferred first.
 * Any Chromium works; this is only about finding one already installed.
 * `DECK_BROWSER` overrides the search for anything not on the list.
 */
const BROWSER_CANDIDATES = [
  "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
];

/** Locate a usable Chromium, or explain how to point the export at one. */
export function resolveBrowser() {
  const override = process.env.DECK_BROWSER;
  if (override) {
    if (!existsSync(override)) {
      throw new Error(`DECK_BROWSER is set to a path that does not exist: ${override}`);
    }
    return override;
  }

  const found = BROWSER_CANDIDATES.find((path) => existsSync(path));
  if (!found) {
    throw new Error(
      "No Chromium-based browser found. Install Brave or Chrome, or set " +
        "DECK_BROWSER to a browser binary.",
    );
  }
  return found;
}

/**
 * How long a page gets to settle before the shot is taken.
 *
 * `--virtual-time-budget` is not a sleep: Chromium runs timers and animations
 * at maximum speed and captures once the budget of *virtual* time is spent, so
 * this is generous without being slow. It has to cover the deck's own fade-in
 * and, on a cold dev server, the route's first compile.
 */
const VIRTUAL_TIME_MS = 10000;

/**
 * Capture one page and return its PNG bytes.
 *
 * A fresh process per page is deliberate rather than wasteful: Chromium's
 * `--screenshot` captures once and exits, and driving one long-lived instance
 * across pages would mean speaking the DevTools protocol — a dependency, and a
 * moving target — to do what a URL parameter already does.
 */
async function capturePage(browser, baseUrl, route, index, dir) {
  const out = join(dir, `page${index + 1}.png`);
  const url = `${baseUrl}${route}?export=1&slideIndex=${index}&stepIndex=0`;

  await execFileAsync(browser, [
    "--headless",
    "--disable-gpu",
    // Without this a scrollbar can steal a strip of the right edge, which on a
    // full-bleed page is a visible seam in the exported slide.
    "--hide-scrollbars",
    // Pin the ratio so a HiDPI display doesn't silently capture at 2×.
    "--force-device-scale-factor=1",
    `--window-size=${CAPTURE_SIZE.width},${CAPTURE_SIZE.height}`,
    `--virtual-time-budget=${VIRTUAL_TIME_MS}`,
    `--screenshot=${out}`,
    url,
  ]);

  return readFile(out);
}

/**
 * Capture every page of a deck in order, returning one PNG buffer per page.
 *
 * Pages are captured sequentially. They could run concurrently, but ten
 * headless Chromium processes against one dev server is how you get a timeout
 * on the slowest page, and the whole run is seconds either way.
 */
export async function captureDeck({ baseUrl, route, pages, onPage = undefined }) {
  const browser = resolveBrowser();
  const dir = await mkdtemp(join(tmpdir(), "dropset-deck-"));

  try {
    const shots = [];
    for (let i = 0; i < pages; i += 1) {
      shots.push(await capturePage(browser, baseUrl, route, i, dir));
      onPage?.(i + 1, pages);
    }
    return shots;
  } finally {
    await rm(dir, { force: true, recursive: true });
  }
}
