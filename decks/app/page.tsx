import Link from "next/link";
import { decks } from "@/lib/decks";
import { ExportButton } from "./ExportButton";

/**
 * The deck index.
 *
 * The page opens with the two things a visitor needs — what this is, and that
 * a deck is interactive — and nothing else. Everything about shortcuts,
 * presenter mode and how the export works is real but rarely wanted, so it
 * sits in a closed disclosure at the bottom. Ahead of the deck list it pushed
 * the decks themselves below a wall of text.
 */
export default function Home() {
  return (
    <main className="mx-auto flex min-h-full max-w-3xl flex-col px-6 py-20 sm:py-28">
      <header className="mb-12">
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
          Talks and demos for Dropset — where currency trades onchain. Open a
          deck and drive it with the arrow keys, or export it to PowerPoint.
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
            {/* A sibling of the card link, not a child: a button nested inside
                an anchor would make a click ambiguous, and this one must not
                navigate. */}
            <div className="px-6 pb-5">
              <ExportButton route={deck.route} title={deck.title} />
            </div>
          </li>
        ))}
      </ul>

      <details className="group mt-12 rounded-xl border border-border bg-muted/20 open:bg-muted/30">
        <summary className="cursor-pointer list-none px-5 py-4 text-sm text-muted-fg transition-colors hover:text-foreground">
          <span className="font-mono text-xs tracking-widest uppercase">
            Presenting and exporting
          </span>
          <span className="ml-2 text-xs text-muted-fg group-open:hidden">+</span>
          <span className="ml-2 hidden text-xs text-muted-fg group-open:inline">
            −
          </span>
        </summary>

        <ul className="flex list-disc flex-col gap-2 px-5 pt-1 pb-5 pl-9 text-sm text-muted-fg marker:text-border">
          <li>
            Arrow keys drive a deck.{" "}
            <span className="font-mono text-foreground">⌘⇧P</span> presenter
            mode and notes,{" "}
            <span className="font-mono text-foreground">⌘⇧O</span> overview,{" "}
            <span className="font-mono text-foreground">⌘⇧E</span> static pages.
          </li>
          <li>
            Those shortcuts have query strings too —{" "}
            <span className="font-mono">?exportMode=true</span> and friends —
            which is the reliable way in, since browsers claim some key
            combinations.
          </li>
          <li>
            Presenter mode syncs over{" "}
            <span className="font-mono">BroadcastChannel</span>: same machine,
            same browser, two windows. It can&apos;t drive someone else&apos;s
            screen.
          </li>
          <li>
            <strong className="text-foreground">Export .pptx</strong> renders
            each page at 3840×2160 into a PowerPoint file — what Google
            Slides&apos; <span className="font-mono">File ▸ Import slides</span>{" "}
            accepts. Slides can&apos;t import a PDF; for one, import here first
            and use <span className="font-mono">File ▸ Download</span>.
          </li>
          <li>
            The same export runs from a checkout as{" "}
            <span className="font-mono text-foreground">pnpm run export</span>.
          </li>
          <li>
            Decks are{" "}
            <a
              href="https://commerce.nearform.com/open-source/spectacle/"
              target="_blank"
              rel="noopener noreferrer"
              className="text-foreground underline underline-offset-4 hover:text-accent"
            >
              Spectacle
            </a>{" "}
            presentations, so the shortcuts are the library&apos;s own.
          </li>
        </ul>
      </details>
    </main>
  );
}
