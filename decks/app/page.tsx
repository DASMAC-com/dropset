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
          Talks and demos for Dropset — forex on Solana. Pick a deck; arrow
          keys drive it, <span className="font-mono text-foreground">p</span>{" "}
          opens presenter mode.
        </p>
      </header>

      <ul className="flex flex-col gap-4">
        {decks.map((deck) => (
          <li key={deck.route}>
            <Link
              href={deck.route}
              className="group block rounded-xl border border-border bg-muted/40 p-6 transition-colors hover:border-accent hover:bg-muted"
            >
              <div className="flex items-baseline justify-between gap-4">
                <h2 className="text-xl font-medium text-foreground transition-colors group-hover:text-accent">
                  {deck.title}
                </h2>
                <time className="shrink-0 font-mono text-xs text-muted-fg">
                  {deck.updated}
                </time>
              </div>
              <p className="mt-2 text-muted-fg">{deck.subtitle}</p>
            </Link>
          </li>
        ))}
      </ul>
    </main>
  );
}
