import { NextRequest } from "next/server";
import { captureDeck } from "@/scripts/capture.mjs";
import { buildPptx } from "@/scripts/pptx.mjs";
import { decks } from "@/lib/decks";

/**
 * On-the-fly deck export: `GET /api/export?deck=/demo-v1` returns a `.pptx`.
 *
 * This is the same engine the `pnpm run export` CLI drives — the CLI is a thin
 * wrapper that starts a server and calls this route, so there is exactly one
 * implementation and the download link cannot drift from the command.
 *
 * The route drives a headless browser over this same server, which is why it
 * is pinned to the Node runtime and marked dynamic: it needs `child_process`,
 * and its output depends on rendering rather than on anything cacheable.
 */
export const runtime = "nodejs";
export const dynamic = "force-dynamic";

/**
 * A capture run is ten sequential headless page loads, plus unpacking Chromium
 * on a cold start — far past the platform's ten-second default.
 *
 * 60s is the ceiling every Vercel plan allows; raise it toward 300 on a plan
 * that permits it if a deck ever grows enough to need it.
 */
export const maxDuration = 60;

/** `Colosseum Cohort 5 Demo Day` → `colosseum-cohort-5-demo-day`. */
const slugify = (title: string) =>
  title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");

export async function GET(request: NextRequest) {
  const route = request.nextUrl.searchParams.get("deck") ?? decks[0]?.route;
  const deck = decks.find((candidate) => candidate.route === route);

  if (!deck) {
    return Response.json(
      {
        error: `Unknown deck: ${route}`,
        available: decks.map((candidate) => candidate.route),
      },
      { status: 404 },
    );
  }

  try {
    // Capture against this server's own origin, so the export always reflects
    // the deployment serving it rather than some configured other one.
    const pages = await captureDeck({
      baseUrl: request.nextUrl.origin,
      route: deck.route,
      pages: deck.pages,
    });

    const pptx = await buildPptx(pages);

    // `Response` takes a Uint8Array; a Node Buffer is one at runtime but is
    // not assignable to `BodyInit`, and the view shares the same memory.
    return new Response(new Uint8Array(pptx), {
      headers: {
        "Content-Type":
          "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "Content-Disposition": `attachment; filename="${slugify(deck.title)}.pptx"`,
        // The deck changes whenever its source does, and the whole point is
        // getting the current pixels.
        "Cache-Control": "no-store",
      },
    });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    return Response.json({ error: "Export failed", detail }, { status: 500 });
  }
}
