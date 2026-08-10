"use client";

import { useState } from "react";

/**
 * Downloads a deck as `.pptx`, showing progress while it is built.
 *
 * A plain link would be simpler, but the export is not a static file: the
 * server renders every page in a headless browser first, which takes several
 * seconds with no visible sign that anything is happening. A bare anchor
 * spends that time looking broken, and an impatient second click starts a
 * second render. So the request is made in JS, the button reports what it is
 * doing, and the result is saved from memory once it arrives.
 */

type Status = "idle" | "working" | "error";

/** Pull the server's filename out of `Content-Disposition`. */
const filenameFrom = (disposition: string | null, fallback: string) =>
  (disposition && /filename="(.+?)"/.exec(disposition)?.[1]) || fallback;

/** Save a blob under `name` by clicking a link that never enters the document. */
function saveBlob(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  // Revoking immediately can race the download in some browsers; a tick is
  // enough for the click to have been handed off.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function ExportButton({ route, title }: { route: string; title: string }) {
  const [status, setStatus] = useState<Status>("idle");

  async function run() {
    if (status === "working") return;
    setStatus("working");

    try {
      const response = await fetch(`/api/export?deck=${encodeURIComponent(route)}`);
      if (!response.ok) throw new Error(`Export failed (${response.status})`);

      const blob = await response.blob();
      saveBlob(blob, filenameFrom(response.headers.get("content-disposition"), "deck.pptx"));
      setStatus("idle");
    } catch {
      setStatus("error");
    }
  }

  const working = status === "working";

  return (
    <div className="flex items-center gap-3">
      <button
        type="button"
        onClick={run}
        disabled={working}
        aria-busy={working}
        className="inline-flex items-center gap-2 rounded-md border border-border px-3 py-1.5 font-mono text-xs text-muted-fg transition-colors hover:border-accent hover:text-accent disabled:cursor-wait disabled:opacity-70 disabled:hover:border-border disabled:hover:text-muted-fg"
      >
        {working && (
          <span
            aria-hidden
            className="h-3 w-3 animate-spin rounded-full border border-current border-t-transparent"
          />
        )}
        {working ? "Building…" : "Export .pptx"}
      </button>

      {status === "error" && (
        <span role="status" className="text-xs text-muted-fg">
          Export failed — check the server log.
        </span>
      )}
      {working && (
        <span role="status" className="text-xs text-muted-fg">
          Rendering {title} — a few seconds.
        </span>
      )}
    </div>
  );
}
