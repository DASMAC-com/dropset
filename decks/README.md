<!-- cspell:word kbar -->

<!-- cspell:word letterboxed -->

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
  `frontend/app/globals.css` and reshaped into a Spectacle theme. It also
  fixes the **slide box** (`DECK_SIZE`, 1920×1080): Spectacle takes the
  native size from the theme rather than a `<Deck>` prop, and its own
  default (1366×768) is *not* exactly 16:9. Every explicit size in a deck
  is in those slide units, so a value lifted from a 1366-era deck reads a
  third too small.

- Typography is **Inter** (primary) and **Space Mono** (mono/tag), the
  DASMAC brand faces, loaded in `app/layout.tsx` through
  `next/font/google` — self-hosted at build time, so no font files are
  committed. Space Mono is what the product website types in, which is
  why it beats the JetBrains Mono named on the DASMAC brand page.

- `components/` — pieces shared across *decks*, added when a second deck
  needs one. There are none today: a deck's own building blocks live
  beside it (`app/demo-v1/DemoDeck.tsx`), and the demo-video badge that
  used to live here is gone, since decks are now **static-image-only**
  (see "Write a deck").

- `scripts/fetch-remote-assets.mjs` — the remote-image mirror, run on the
  `predev` / `prebuild` hooks; see `remote-assets.json` below.

- `public/` — deck assets, from three sources:

  - Everything in the repo-root `brand-assets/` — the single source of
    truth for **every** brand asset, including ones only one app renders
    — is **copied** in by `../brand-assets/copy-brand-assets.mjs` on the
    `predev` / `prebuild` hooks, so the brand assets stay DRY without a
    symlink escaping the deck's Vercel Root Directory. The script copies
    the directory rather than a list, so a new asset is a drop-in file,
    and the whole folder goes to every app rather than a per-app subset.

  - `public/remote/` is **mirrored** from the URLs in
    `remote-assets.json` by `scripts/fetch-remote-assets.mjs`, on the
    same two hooks — images we don't hold a copy of: the two team
    headshots the marketing site serves, the four partner logos on the
    growth page, and the three permissioned-rail marks on the
    open-venue page.

  - `public/screens/` holds our own screen captures, which are
    **committed**: nothing external hosts them, so there's nothing to
    mirror.

    Two things to do to a capture before committing it. **Scale it to
    about twice the width its slide renders it at** — the print path
    renders at the 1920×1080 slide box, so anything beyond that is
    weight the deck can never show. Then **quantize it to a 256-color
    palette**: these are dark product-UI captures (a few greys, two
    accent hues, small flag icons), so they sit well inside 256 colors,
    and it is a ~90% saving that also keeps every file under the repo's
    500KB-per-file commit limit. The raw captures behind the current set
    were 4.4MB; committed, they are 437KB, with text and flags still
    crisp. `sips` (built in) resizes; Pillow does the crop and the
    quantize.

  The first two are generated, so `public/`'s entries are gitignored with
  a carve-out for the committed `public/screens/` — see `.gitignore`.

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
`⌘⇧E` for export and `⌘⇧R` for print. A bare `p` does nothing, which reads
as presenter mode being broken when it isn't. Every mode also has a query
string (`?exportMode=true`, `?presenterMode=true`, …), which is worth
knowing because `⌘⇧R` collides with the browser's own hard-reload.

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

Nothing in a deck depends on a live network or a live cluster — decks are
static-image-only — so a room with no connection presents fine once the
page is loaded.

## Export to Google Slides

The accelerator combines every team's slides into one meta-deck, which is a
Google Slides file. Slides' `File ▸ Import slides` accepts only `.pptx` /
`.ppt` / an existing Slides deck — it **cannot import a PDF at all** — so
`.pptx` is the deliverable, not a convenience.

Two ways to get one, both the same code path:

```sh
pnpm run export              # the first deck in the registry
pnpm run export -- /demo-v1  # a specific deck route
```

writes `out/<deck-title>.pptx`, or click **Export .pptx** on the landing
page, which hits `GET /api/export?deck=/demo-v1`. The command is a thin
client of that route, so a download and a command-line export are the same
bytes.

For a **PDF**, import the `.pptx` into Slides and use `File ▸ Download`.
Going straight to PDF is not worth a separate path: Slides is the only
destination that matters, and it needs the `.pptx` anyway.

### How it works, and why not print-to-PDF

The exporter loads each page in a headless Chromium — `?slideIndex=N`, one
page per shot — and packs the screenshots into a `.pptx` as one full-bleed
picture per slide (`scripts/capture.mjs` and `scripts/pptx.mjs`). It finds
Brave, Chrome, Chromium or Edge automatically;
`DECK_BROWSER=/path/to/browser` overrides that.

Pages lay out in the deck's own 1920×1080 space but are captured at a device
pixel ratio of 2, so the images are **3840×2160**. Raising the pixel ratio
rather than the viewport is deliberate: a bigger viewport would give
Spectacle a bigger box and change the layout, while a higher ratio renders
the same layout with more samples. At 1× the result looked grainy once
Google Slides had rescaled it.

Screenshots, rather than Spectacle's own export mode plus print-to-PDF,
because that route is broken here and dead-ends anyway. `⌘⇧R` print mode
merges Spectacle's print theme — backdrop white, headings black, body grey —
which inverts a deck built on a dark backdrop and erases the Dropset
wordmark outright, since that is an opaque PNG shown with
`mix-blend-mode: screen` and screening over white returns white. `⌘⇧E`
export mode keeps the deck's colors but still yields a PDF, and a PDF still
has to be rasterized before Slides will take it, with a tool this repo does
not carry. Capturing the live page skips both problems: the pixels come from
the same renderer the deck is reviewed in, and they are already images.

What the pipeline asks of a deck is what the theme already enforces: the
slide box is exactly 16:9 (`DECK_SIZE`), so nothing letterboxes on import.
Nothing is stripped for export — a captured page is the page, progress dots
and all.

A captured page is measurably the page: content sits ~5% in from the left
edge and the footer ~9% up from the bottom, in both the export and the
browser — slightly *tighter* in the export, since a browser window that
isn't exactly 16:9 letterboxes on top of that. If those margins look wrong,
they are the deck's own layout (`SlideBody`, the footer's padding) and the
place to change them is the deck, not the exporter.

Two things to know when a deck changes:

- **Update `pages` in `lib/decks.ts`** when you add or remove a slide. A
  browser given an out-of-range `slideIndex` re-renders the last page instead
  of failing, so a count that is too high yields silent duplicates and one
  too low silently truncates.
- **Check every page after a layout change.** A page that overflows its box
  is clipped in the output, silently. The eyebrow is the tell: if a page's
  kicker is missing, that page overflowed.

## Add a deck

1. Create `app/<public-route>/page.tsx` + `<Deck>.tsx` (copy `demo-v1`).
1. Add an entry to `lib/decks.ts`.

That entry's date is `presented` — **the date the deck is given**, not the
date it was last edited. It used to be the latter, which meant the landing
page showed whenever someone last remembered to bump it; an event date is
fixed, so it needs no upkeep and answers the question a reader actually
has.

## Check

```sh
make decks-build
```

The production build, which is what CI gates on — a step in the `lint`
workflow, since the `test` workflow path-filters `decks/**` out. It
type-checks every deck and runs the asset hooks, so a broken deck (or an
asset that can't be sourced) fails the merge queue rather than surfacing
mid-presentation.

Run it **once before committing** — it is a pre-commit check, not an
inner-loop tool. It installs, wipes `.next`, and does a full optimizing
build, so firing it after each micro-edit during layout iteration costs
minutes and a log per edit for a change `make decks` above hot-reloads
instantly. Iterate on the dev server; build to check (see
`docs/conventions/context-economy.md`).

## Write a deck

`demo-v1` is reconciled to `demo-v1-spec.md` — the reviewable copy for
that pitch, and the source of truth for it: eight pages against a
ten-page cap, one big sentence and one big visual per page, with the
nuance kept off the slides and in that doc's appendices. Edit the spec
first, then the deck. Spoken script belongs in each slide's `<Notes>`
(presenter mode, `⌘⇧P`), never on the slide.

Four rules from that spec bind any deck here, not just `demo-v1`:

- **Static images only.** No embedded video, no gifs, no player. A
  product beat is an interface screenshot with a full-sentence claim over
  it. This is what keeps a deck presentable offline and printable.
- **Full sentences on-slide**, never fragment headlines — the deck gets
  read without the talk more often than it gets presented.
- **No competitor names or logos on a slide.** Naming one hands it the
  frame; make the argument in type and keep the names in the spec's
  appendix, for conversation.
- **Logos are argued, never listed.** A partner mark is captioned with
  what that company is to us. A competitor mark appears only where it is
  visibly the case being argued *against* (`demo-v1` red-outlines three
  on one page, with the Dropset wordmark opposite as the answer) — a
  neutral row of competitor logos hands them the frame instead.

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
