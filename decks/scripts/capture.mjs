// cspell:word screenshotting
// cspell:word networkidle
// cspell:word downsamples

import { existsSync } from "node:fs";
import puppeteer from "puppeteer-core";

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
 *
 * The browser is driven over the DevTools protocol rather than by spawning
 * `--screenshot` per page. That is what lets one browser serve all ten pages —
 * an order of magnitude less overhead — and, more importantly, it is the only
 * form that works on a serverless host, where there is no browser to spawn and
 * the binary has to be unpacked and launched explicitly.
 */

/**
 * The deck's design space, and therefore the capture viewport. Kept in step
 * with `DECK_SIZE` in `theme/tokens.ts` — capturing at the size the deck is
 * laid out for means no scaling, so text is rendered rather than resampled.
 */
const CAPTURE_SIZE = { width: 1920, height: 1080 };

/**
 * Device pixel ratio for the capture, so the PNGs come out at 3840×2160 while
 * the page still lays out in its 1920×1080 design space.
 *
 * Both halves of that matter. Raising the *viewport* instead would give
 * Spectacle a bigger box to fit and change the layout; raising the pixel ratio
 * renders the same layout with more samples, which is what makes text and the
 * screenshot artwork crisp rather than resampled.
 *
 * 2× is the ratio the destination actually justifies: 3840×2160 is exactly a 4K
 * projector, so the deck arrives at 1:1 on the best screen it will meet and is
 * cleanly downscaled on any lesser one.
 *
 * This was 3× for a while, on the reasoning that Google Slides degraded the
 * import and more samples would survive it. That reasoning was wrong, and the
 * measurement that settled it is worth not repeating: exporting a deck,
 * importing it to Slides, downloading it back as `.pptx`, and comparing the
 * embedded images found all ten pages **byte-identical** — same dimensions,
 * same PNG, same checksum. Slides neither re-encodes nor downsamples what it
 * stores. The softness is in how Slides *renders* a slide (in the editor canvas
 * and in Present mode alike), which no amount of source resolution reaches. So
 * the ratio is set by the projector, and 3× was 4.7MB of pixels nothing
 * downstream could ever show.
 */
const DEFAULT_CAPTURE_SCALE = 2;

/**
 * `DECK_CAPTURE_SCALE` overrides the ratio, so the same deck can be exported at
 * several resolutions and the results compared side by side — which is how the
 * default above was chosen, and the only honest way to revisit it.
 */
const CAPTURE_SCALE = Number(process.env.DECK_CAPTURE_SCALE ?? DEFAULT_CAPTURE_SCALE);

/**
 * Chromium caps a capture surface at 16384px on a side, and it does not report
 * crossing that — it returns a clipped or blank shot, so the export "succeeds"
 * with broken pages. Against the fixed 1920×1080 viewport the limit lands at
 * 8.5×, so the bound sits at 8× (15360×8640) and the failure is loud.
 */
const MAX_CAPTURE_SCALE = 8;

if (!Number.isFinite(CAPTURE_SCALE) || CAPTURE_SCALE <= 0 || CAPTURE_SCALE > MAX_CAPTURE_SCALE) {
  throw new Error(
    `DECK_CAPTURE_SCALE must be a number in (0, ${MAX_CAPTURE_SCALE}], ` +
      `got ${JSON.stringify(process.env.DECK_CAPTURE_SCALE)}`,
  );
}

/**
 * Locally-installed Chromium builds, most-preferred first. Any Chromium works;
 * this is only about finding one already on the machine. `DECK_BROWSER`
 * overrides the search for anything not on the list.
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

/**
 * True when running on Vercel's serverless runtime, which has no browser
 * installed — the binary ships as a dependency and is unpacked per cold start.
 */
const isServerless = () =>
  Boolean(process.env.AWS_LAMBDA_FUNCTION_NAME || process.env.VERCEL);

/**
 * Launch a browser, wherever this is running.
 *
 * The serverless branch is imported lazily rather than at module scope: the
 * package unpacks a ~50MB brotli-compressed Chromium on load, which is pure
 * cost for every local run and every request that never exports.
 */
async function launch() {
  if (isServerless()) {
    const { default: chromium } = await import("@sparticuz/chromium");
    return puppeteer.launch({
      args: chromium.args,
      executablePath: await chromium.executablePath(),
      headless: true,
    });
  }

  const override = process.env.DECK_BROWSER;
  if (override && !existsSync(override)) {
    throw new Error(`DECK_BROWSER is set to a path that does not exist: ${override}`);
  }

  const executablePath = override ?? BROWSER_CANDIDATES.find((path) => existsSync(path));
  if (!executablePath) {
    throw new Error(
      "No Chromium-based browser found. Install Brave or Chrome, or set " +
        "DECK_BROWSER to a browser binary.",
    );
  }

  return puppeteer.launch({ executablePath, headless: true });
}

/**
 * How long a page gets to finish loading and settle before the shot is taken.
 * It has to cover the deck's fade-in and, on a cold dev server, the route's
 * first compile.
 */
const NAVIGATION_TIMEOUT_MS = 60000;
const SETTLE_MS = 600;

/**
 * Capture every page of a deck in order, returning one PNG buffer per page.
 *
 * One browser and one tab throughout: each page is a navigation, which is
 * cheap, and running ten tabs at once against a single dev server is how the
 * slowest of them ends up timing out.
 */
export async function captureDeck({ baseUrl, route, pages, onPage = undefined }) {
  const browser = await launch();

  try {
    const page = await browser.newPage();
    await page.setViewport({ ...CAPTURE_SIZE, deviceScaleFactor: CAPTURE_SCALE });

    const shots = [];
    for (let i = 0; i < pages; i += 1) {
      await page.goto(`${baseUrl}${route}?slideIndex=${i}&stepIndex=0`, {
        waitUntil: "networkidle0",
        timeout: NAVIGATION_TIMEOUT_MS,
      });
      // `networkidle0` means the requests stopped, not that the deck has
      // finished its entrance transition.
      await new Promise((resolve) => setTimeout(resolve, SETTLE_MS));

      shots.push(await page.screenshot({ type: "png" }));
      onPage?.(i + 1, pages);
    }

    return shots;
  } finally {
    await browser.close();
  }
}
