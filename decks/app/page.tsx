"use client";

import Link from "next/link";
import { useState } from "react";
import { decks } from "@/lib/decks.mjs";

/**
 * How a deck opens, offered as a choice ahead of the deck list.
 *
 * Spectacle reads its mode from the query string at mount — `useModes` parses
 * `window.location.search` before anything renders — so the choice only has to
 * decide which href the cards point at. That keeps them real links: a
 * middle-click, a copied URL, or a bookmark all carry the choice with them,
 * which a click handler calling `router.push` would have thrown away.
 *
 * The labels describe what the reader *gets* rather than naming the mode.
 * "Presenter mode" is accurate and actively misleading here: a visitor who
 * isn't presenting reads it as somebody else's setting and switches it off,
 * losing the talk track — which is the most valuable thing on the page for
 * exactly that reader.
 */
const DECK_MODES = [
  {
    id: "notes",
    label: "Show presenter notes",
    description: "The talk track that goes with each slide, alongside it.",
    // Spectacle's own parameter, the one its ⌘⇧P shortcut sets — the same
    // door as the shortcut, with a handle a visitor will actually pull.
    search: "?presenterMode=true",
  },
  {
    id: "slides",
    label: "Slides only",
    description: "The deck full-screen, the way an audience sees it.",
    search: "",
  },
] as const;

type DeckModeId = (typeof DECK_MODES)[number]["id"];

/**
 * The deck index.
 *
 * The page opens with the three things a visitor needs — what this is, that a
 * deck is interactive, and how it will open — and nothing else. The remaining
 * detail about shortcuts and presenting is real but rarely wanted, so it sits
 * in a closed disclosure at the bottom. Ahead of the deck list it pushed the
 * decks themselves below a wall of text.
 */
export default function Home() {
  // Notes are on by default, and re-default on every visit rather than being
  // remembered: the talk track is the reason these decks are published at all,
  // so each arrival should land on it regardless of what the last one chose.
  const [modeId, setModeId] = useState<DeckModeId>(DECK_MODES[0].id);
  const activeMode = DECK_MODES.find((m) => m.id === modeId) ?? DECK_MODES[0];

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
          deck and drive it with the arrow keys.
        </p>
      </header>

      <fieldset className="mb-8">
        <legend className="mb-3 font-mono text-xs tracking-widest text-muted-fg uppercase">
          Opening a deck
        </legend>
        <div className="flex flex-col gap-3 sm:flex-row">
          {DECK_MODES.map((mode) => (
            <label
              key={mode.id}
              className={`flex flex-1 cursor-pointer items-start gap-3 rounded-xl border px-4 py-3 transition-colors ${
                mode.id === modeId
                  ? "border-accent bg-muted/50"
                  : "border-border bg-muted/20 hover:border-accent/50"
              }`}
            >
              <input
                type="radio"
                name="deck-mode"
                value={mode.id}
                checked={mode.id === modeId}
                onChange={() => setModeId(mode.id)}
                className="mt-1 accent-accent"
              />
              <span>
                <span className="block text-sm text-foreground">
                  {mode.label}
                </span>
                <span className="mt-0.5 block text-xs text-muted-fg">
                  {mode.description}
                </span>
              </span>
            </label>
          ))}
        </div>
      </fieldset>

      <ul className="flex flex-col gap-4">
        {decks.map((deck) => (
          <li
            key={deck.route}
            className="rounded-xl border border-border bg-muted/40 transition-colors hover:border-accent"
          >
            <Link
              href={`${deck.route}${activeMode.search}`}
              className="group block p-6 transition-colors hover:bg-muted/60"
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
          </li>
        ))}
      </ul>

      <details className="group mt-12 rounded-xl border border-border bg-muted/20 open:bg-muted/30">
        <summary className="cursor-pointer list-none px-5 py-4 text-sm text-muted-fg transition-colors hover:text-foreground">
          <span className="font-mono text-xs tracking-widest uppercase">
            Presenting
          </span>
          <span className="ml-2 text-xs text-muted-fg group-open:hidden">+</span>
          <span className="ml-2 hidden text-xs text-muted-fg group-open:inline">
            −
          </span>
        </summary>

        <ul className="flex list-disc flex-col gap-2 px-5 pt-1 pb-5 pl-9 text-sm text-muted-fg marker:text-border">
          <li>
            Arrow keys drive a deck.{" "}
            <span className="font-mono text-foreground">⌘⇧P</span> toggles the
            notes from inside a deck, whichever way you opened it,{" "}
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
            A PowerPoint file for Google Slides&apos;{" "}
            <span className="font-mono">File ▸ Import slides</span> is built
            from a checkout with{" "}
            <span className="font-mono text-foreground">pnpm run export</span>.
            It renders each page at 3840×2160 and needs a local browser, so it
            isn&apos;t something this site can do for you.
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
