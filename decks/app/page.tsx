import Link from "next/link";
import { decks } from "@/lib/decks";

export default function Home() {
  return (
    <main className="mx-auto flex min-h-full max-w-3xl flex-col px-6 py-20 sm:py-28">
      <header className="mb-16">
        <div className="mb-6 flex items-center gap-3">
          <span className="relative flex h-3 w-3">
            <span className="absolute inline-flex h-full w-full rounded-full bg-brand opacity-40" />
            <span className="relative inline-flex h-3 w-3 rounded-full bg-brand" />
          </span>
          <span className="font-mono text-sm tracking-widest text-muted-fg uppercase">
            Dropset Decks
          </span>
        </div>
        <h1 className="text-4xl font-semibold tracking-tight sm:text-5xl">
          Presentation decks
        </h1>
        <p className="mt-4 max-w-xl text-lg text-muted-fg">
          Talks and demos for Dropset — where currency trades onchain. Pick a
          deck; arrow keys drive it,{" "}
          <span className="font-mono text-foreground">⌘⇧P</span> opens
          presenter mode with the speaker notes,{" "}
          <span className="font-mono text-foreground">⌘⇧O</span> the slide
          overview, and{" "}
          <span className="font-mono text-foreground">⌘⇧E</span> export mode —
          every slide as a static page.
        </p>
        <p className="mt-3 max-w-xl text-muted-fg text-sm">
          Every deck is a{" "}
          <a
            href="https://commerce.nearform.com/open-source/spectacle/"
            target="_blank"
            rel="noopener noreferrer"
            className="text-foreground underline underline-offset-4 hover:text-accent"
          >
            Spectacle
          </a>{" "}
          presentation, so those shortcuts are the library&apos;s own. Each has
          a query string too —{" "}
          <span className="font-mono">?exportMode=true</span> and friends —
          which is the reliable way in, since the browser claims some of the
          key combinations for itself.
        </p>
        <p className="mt-3 max-w-xl text-muted-fg text-sm">
          Presenter mode syncs to the audience window over the browser&apos;s{" "}
          <span className="font-mono">BroadcastChannel</span> — same machine,
          same browser, two windows. It does not drive a deck on someone
          else&apos;s screen.
        </p>
        <p className="mt-3 max-w-xl text-muted-fg text-sm">
          To hand a deck to someone else, use <strong>Export .pptx</strong>{" "}
          below. It screenshots every page at 1920×1080 and packs them into a
          PowerPoint file, which is what Google Slides&apos;{" "}
          <span className="font-mono">File ▸ Import slides</span> accepts —
          Slides cannot import a PDF at all. For a PDF, import to Slides and use{" "}
          <span className="font-mono">File ▸ Download</span>. Do not print
          from&nbsp;
          <span className="font-mono">⌘⇧R</span>: Spectacle&apos;s print mode
          swaps in a light theme, which inverts a deck built on a dark backdrop
          and drops any artwork that relies on blend modes.
        </p>
      </header>

      <ul className="flex flex-col gap-4">
        {decks.map((deck) => (
          <li
            key={deck.route}
            className="rounded-xl border border-border bg-muted/40 transition-colors hover:border-accent"
          >
            <Link
              href={deck.route}
              className="group block p-6 pb-4 transition-colors hover:bg-muted/60"
            >
              <div className="flex items-baseline justify-between gap-4">
                <h2 className="text-xl font-medium text-foreground transition-colors group-hover:text-accent">
                  {deck.title}
                </h2>
                <time className="shrink-0 font-mono text-xs text-muted-fg">
                  {deck.presented}
                </time>
              </div>
              <p className="mt-2 text-muted-fg">{deck.subtitle}</p>
            </Link>
            {/* A sibling of the card link, not a child: an anchor nested inside
                another anchor is invalid, and the browser would resolve a click
                to whichever it liked. */}
            <div className="flex items-center gap-3 px-6 pb-5 text-sm">
              <a
                href={`/api/export?deck=${encodeURIComponent(deck.route)}`}
                className="rounded-md border border-border px-3 py-1.5 font-mono text-xs text-muted-fg transition-colors hover:border-accent hover:text-accent"
              >
                Export .pptx
              </a>
              <span className="text-xs text-muted-fg">
                {deck.pages} pages · takes a few seconds
              </span>
            </div>
          </li>
        ))}
      </ul>
    </main>
  );
}
