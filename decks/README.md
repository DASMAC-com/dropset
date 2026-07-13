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
- `public/` — deck assets. `dropset-wordmark.png` and
  `favicon-with-stroke.svg` are **copied** from the repo-root
  `brand-assets/` — the single source of truth for shared brand assets —
  by `../scripts/copy-brand-assets.mjs` on the `predev` / `prebuild` hooks,
  so the brand assets stay DRY without a symlink escaping the deck's Vercel
  Root Directory. They're generated, so both are gitignored.

Deck routes use **public-facing names** (e.g. `/demo-v1`) — never internal
ticket ids, which must not leak into shareable URLs.

## Develop

```sh
make decks
```

Installs, serves on **<http://localhost:3300>** (port set in the `dev`
script; see the port-allocation table in the repo `Makefile`), and opens
a browser once it's up. Arrow keys drive a deck; `p` opens presenter mode
(speaker notes + next-slide preview); `f` goes fullscreen.

## Add a deck

1. Create `app/<public-route>/page.tsx` + `<Deck>.tsx` (copy `demo-v1`).
1. Add an entry to `lib/decks.ts`.

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
