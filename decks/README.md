<!-- cspell:word kbar -->

# decks

Presentation decks for Dropset, deployed to **decks.dropset.io**. A
standalone Next.js (app router) + [Spectacle] package in the
monorepo workspace, kept separate from `frontend` so its deploy config,
deps, and theme don't fight the product build.

> Pinned to **React 18 / Next 14**: Spectacle (and its transitive deps —
> `react-spring`, `kbar`, `use-resize-observer`) cap at React ≤18, so this
> package runs a React 18 toolchain independent of `frontend`'s React 19.
> There's no shared React runtime — theme tokens are copied constants
> (`theme/tokens.ts`), not imported components — so the versions diverge
> safely.

## Layout

- `app/page.tsx` — landing page; indexes the decks in `lib/decks.ts`.

- `app/<route>/` — one deck per route. The deck itself is a client-only
  Spectacle `<Deck>` (`page.tsx` dynamic-imports it with `ssr: false`).

- `theme/tokens.ts` — Dropset design tokens, mirrored from
  `frontend/app/globals.css` and reshaped into a Spectacle theme.

- `components/` — pieces shared across decks. `DemoVideo.tsx` is the demo
  badge: click it and the recording plays over the whole window, through a
  portal to `document.body` (a Spectacle slide sits under a CSS transform,
  which would otherwise trap a `position: fixed` overlay inside the slide
  box). It swallows arrow keys while open so the deck doesn't advance
  behind the video, and closes on `esc` or a click outside. Each demo
  passes the **source recording's pixel dimensions**, and the overlay is
  sized from them to fill the viewport at that exact shape — YouTube
  chooses playback quality from the rendered player box, so a mis-shaped
  or capped player is what makes a 4K upload play at 720p.

- `scripts/fetch-remote-assets.mjs` — the remote-image mirror, run on the
  `predev` / `prebuild` hooks; see `remote-assets.json` below.

- `public/` — deck assets, from three sources:

  - Everything in the repo-root `brand-assets/` — the single source of
    truth for shared brand assets, currently `dropset-wordmark.png`,
    `dasmac-wordmark.png` and `favicon-with-stroke.svg` — is **copied** in
    by `../brand-assets/copy-brand-assets.mjs` on the `predev` /
    `prebuild` hooks, so the brand assets stay DRY without a symlink
    escaping the deck's Vercel Root Directory. The script copies the
    directory rather than a list, so a new shared asset is a drop-in file.
    They're generated, so each is gitignored.
  - `public/remote/` is **mirrored** from the URLs in
    `remote-assets.json` by `scripts/fetch-remote-assets.mjs`, on the
    same two hooks — images we don't hold a copy of, like the team
    headshots the marketing site serves. Also generated, also gitignored.
  - `public/screens/` holds our own screen captures, which are
    **committed**: nothing external hosts them, so there's nothing to
    mirror.

- `remote-assets.json` — the `<filename>: <url>` manifest the mirror
  reads. Adding a remote image to a slide is one line here plus an
  `<Image src="/remote/<filename>">`. A fetch that fails **exits
  non-zero**, which fails `prebuild` and so the whole build: a deck that
  can't show a face or a logo shouldn't build at all, rather than build
  with a broken image nobody notices until it's on the projector.

  Because the build gates CI, **prefer a URL that can't move**: a GitHub
  raw path pinned to a commit SHA, a brand-kit file on the company's own
  domain, or a token-registry asset with a permanent id. A search-engine
  image cache or a CDN hash rotates, and a rotated URL reddens a required
  check on every PR in the repo. If a mark has no such home, commit it
  under `public/screens/` instead of mirroring it.

Deck routes use **public-facing names** (e.g. `/demo-v1`) — never internal
ticket ids, which must not leak into shareable URLs.

## Develop

```sh
make decks
```

Installs, serves on **<http://localhost:3300>** (port set in the `dev`
script; see the port-allocation table in the repo `Makefile`), and opens
a browser once it's up. Arrow keys drive a deck. The mode shortcuts all
take a modifier — **`⌘⇧P`** (`Ctrl⇧P` off macOS) for presenter mode
(speaker notes + next-slide preview), `⌘⇧O` for the slide overview,
`⌘⇧R` for print. A bare `p` does nothing, which reads as presenter mode
being broken when it isn't.

## Presenting

Present from **decks.dropset.io** or from `make decks` — they behave
identically, because a deck is client-only (`page.tsx` dynamic-imports it
with `ssr: false`) and there is no server-side deck state to differ.

Presenter mode syncs to the audience window over the browser's
`BroadcastChannel`, which is scoped to **one origin in one browser on one
machine**. So the working setup is two windows on the presenting laptop —
notes on your screen, the deck on the projector — and **a second machine
cannot remote-control the first**. Opening presenter mode elsewhere just
gives that machine its own independent copy of the deck. Nothing about
deploying changes this, and nothing about running locally fixes it; for
remote control you'd need a shared backend the package doesn't have.

## Add a deck

1. Create `app/<public-route>/page.tsx` + `<Deck>.tsx` (copy `demo-v1`).
1. Add an entry to `lib/decks.ts`.

## Check

```sh
make decks-build
```

The production build, which is what CI gates on — a step in the `lint`
workflow, since the `test` workflow path-filters `decks/**` out. It
type-checks every deck and runs the asset hooks, so a broken deck (or an
asset that can't be sourced) fails the merge queue rather than surfacing
mid-presentation.

## Write a deck

`demo-v1` is reconciled to `demo-v1-spec.md` — the reviewable copy for
that pitch, and the source of truth for it: eight pages against a
ten-page cap, one big sentence and one big visual per page, with the
nuance kept off the slides and in that doc's appendices. Edit the spec
first, then the deck. Spoken script belongs in each slide's `<Notes>`
(presenter mode, `⌘⇧P`), never on the slide.

## Deploy

A dedicated Vercel project (not the `frontend` project) with **Root
Directory = `decks/`**. `vercel.json` gates deploys to `main` only,
mirroring `frontend`. The gate uses `"**": false` (not `"*"`): minimatch
`*` stops at a `/`, so `"*"` never matches slash-bearing merge-queue
branches (`gh-readonly-queue/main/pr-…`) and Vercel would still preview
them — `"**"` spans the slashes. The custom domain `decks.dropset.io` is
mapped in
Vercel with a `CNAME decks -> cname.vercel-dns.com` DNS record. Creating
that Vercel project + DNS record is a one-time out-of-band step.

[spectacle]: https://commerce.nearform.com/open-source/spectacle/
