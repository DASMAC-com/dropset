<!-- cspell:word cofounded -->

<!-- cspell:word composably -->

<!-- cspell:word Dragonfly -->

<!-- cspell:word emojicoin -->

<!-- cspell:word fundraise -->

<!-- cspell:word Mert -->

<!-- cspell:word steelman -->

<!-- cspell:word verticalized -->

<!-- cspell:word wordmarks -->

# Demo-day pitch spec — `demo-v1`

The **copy** for the ~2-minute pitch at **Colosseum Cohort 5 Demo Day**,
written to be reviewed and edited *before* it's turned into slides. This
is the script and page plan; the built deck lives at `app/demo-v1/` and
should follow this doc, not the other way around. Drop it into Google
Docs, get edits from others, then reconcile the deck to match.

Sections are ordered for Google Docs toggles: **1. Slide contents**
(the actual copy) first, **2. Presentation appendices** (the off-slide
Q&A material) next, and **3. Formatting / structure rules** (how to
read this, the design principles, the reference structure) last.

This is **outline v2** — the first deck went out for review, and this
revision reworks it against the round-1 feedback. What changed:

- Full sentences on every slide, in place of fragment headlines.
- **Static images only.** The two recorded demos and the click-to-play
  badge are gone; product beats are interface screenshots.
- **Nine pages, not eight.** The swap flow and the market data we curate
  are now separate beats, because the second one is what sets up the
  eCLOB: sort every currency by liquidity and the long tail is empty.
- The eCLOB page leads with what the design *gives you*, and its payoff
  visual is the book rendered on the frontend rather than a strip of
  thumbnails.
- A **growth roadmap** page (three beats in time order), answering the
  revenue question the first deck left implied.
- The "why this will fail" / "why it won't" pair **collapsed into one
  page**, and the asterisk device is retired.
- **No competitor names or logos anywhere.** Partner logos stay — those
  are companies we're doing customer development with, and the marks are
  the point.

______________________________________________________________________

## 1. Slide contents

### The 2-minute narrative (continuous read)

The through-line, so the story reads as one piece before it's cut into
pages:

> Dropset is where currency trades onchain, built by DASMAC. Foreign
> exchange is the biggest market on earth — over nine trillion dollars a
> day — but it trades only 24/5, its liquidity is fragmented across
> obfuscated over-the-counter desks, and less than ten percent of the
> world's currencies are even available on Solana today. This already
> works: Dropset is live on mainnet, and you open the picker, type the
> currency you want, and swap. Settlement is atomic, the ramps are near
> instant, and the venue never closes; Solana is the start, not the end,
> because it's the most moneyness-conducive environment onchain.
> Alongside that we curate the market data for every currency onchain —
> and when you sort by liquidity the story tells itself, because a
> handful of pairs are deep and the whole long tail is dry. Those are the
> currencies we're here to make liquid, and that needs a venue. Making a
> market onchain used to be prohibitively expensive, so we built one that
> fits: the eCLOB gives order-book transparency with propAMM efficiency,
> where repricing the whole book costs forty-seven compute units and
> reshaping the ladder fifty-nine. Our vaults bootstrap a public
> liquidity flywheel, and it's a two-sided market we're already doing the
> customer development on — upstream the issuers who need their currency
> to trade, downstream the payments companies who need to buy FX to
> settle. Each stage of the roadmap funds the next: DASMAC leads the
> liquidity now, protocol fees accrue value as the market matures, and
> derivatives are the expansion once spot is nailed. FX's end consumers
> need an open system, and permissioned liquidity isn't public — Dropset
> is open, neutral, and composable. Dropset is built by people who have
> built exchanges. Dropset — where currency trades onchain.

### Page-by-page

#### Page 1 — Title · ~5s

- **On-slide:** Where currency trades onchain. Beneath it: Built by
  DASMAC.
- **Visual:** The Dropset wordmark, then the **DASMAC company banner**
  (`brand-assets/dasmac-banner-wide.png` — the mountains, with the
  "distributed atomic state machine algorithms corporation" tag) across
  the bottom. Uncaptioned: it's brand art, not a figure, and a caption
  explaining what a banner is would undercut it. There is deliberately
  **no Dropset protocol counterpart** — one banner reads as a signature,
  two read as a comparison.
- **Spoken:** "Dropset is where currency trades onchain, built by
  DASMAC."
- **Note:** "Built by DASMAC", not "courtesy of" — the credit is
  authorship, and it carries the company/protocol distinction from the
  first frame. Solana is **not** mentioned here; the old "Forex on
  Solana" line implied a boundary the deck no longer wants. There is
  **no separate closing slide** — the deck ends on the team page.

#### Page 2 — The gap · ~15s

- **On-slide:** Foreign exchange is the biggest market on earth. Then
  three supporting facts: it trades over \$9 trillion a day; its
  liquidity is fragmented across obfuscated over-the-counter desks; less
  than 10% of the world's currencies are available on Solana today.
- **Visual:** A **meter** — 14 filled of 162 — over the currencies count
  from our own site, captioned `dropset.io/currencies`.
- **Spoken:** "Foreign exchange is the biggest market on earth — over
  nine trillion dollars a day. But it only trades 24/5, its liquidity is
  fragmented across obfuscated over-the-counter desks, and less than ten
  percent of the world's currencies are even available on Solana today:
  fourteen out of a hundred and sixty-two, and that count is live on our
  own site. Every currency should be connectable to every other one, and
  that's what we're building."
- **Note:** Frame it as **gap plus upside**, never a market-size slide.
  The every-currency vision beat starts here, worded as **connection,
  not issuance** — Dropset does not issue currencies; issuers create them
  and Dropset is where they trade. The ~\$9T/day figure needs no citation
  (it isn't disputed at pitch-deck level), but the currency count keeps
  its attribution because it's ours and it's checkable. **Do not invent a
  Solana volume-share percentage.**
- **Note on the two visuals:** they do different jobs and both are
  needed. A single ratio against a limit is a **meter**, not a pie of two
  slices — the empty part of the track *is* the message. The screenshot
  under it is the **citation**: our own page, showing the same number,
  with the URL, so it's verifiable rather than asserted.

#### Page 3 — Live today · ~12s

- **On-slide:** Dropset settles real currency trades on mainnet today.
- **Visual:** The swap flow as three numbered stills — open the currency
  picker, type the currency you want, swap and it settles. The first two
  stack in one column (both short, both sequential); the settled trade
  gets its own column, since that capture is tall and is the payoff.
- **Spoken:** "This already works. Dropset is live on mainnet today,
  clearing real trades: you open the picker, type the currency you want,
  and swap. Settlement is atomic, the ramps are near instant, and the
  venue never closes. Solana is the start, not the end — it's the most
  moneyness-conducive environment onchain."
- **Note:** The *why onchain matters* beat lives here, spoken. Keep the
  claim exact: today we clear by routing through aggregators and sourcing
  existing liquidity. Don't assert "most liquid". The globe is **not**
  the way in any more — an earlier draft framed the flow as picking a
  country off the globe, which isn't how anyone actually uses it; the
  globe appears in the third capture as the route being drawn, which is
  what it's for.

#### Page 4 — Every currency, in one place · ~15s

- **On-slide:** We curate the market data for every currency onchain —
  and the long tail has no liquidity at all.
- **Visual:** Three captures of the same table. Left column: grouped by
  country. Right column: sorted by liquidity, with the tail of that same
  sort underneath it, captioned "sorted by liquidity — and the tail is
  empty".
- **Spoken:** "Dropset already settles trades on mainnet by sourcing
  existing liquidity from the other venues — and alongside that we curate
  the market data for every currency onchain: price, volume, market cap,
  liquidity, holders, grouped by country or sorted however you want. Sort
  by liquidity and the story tells itself. A handful of pairs are deep,
  and then the long tail is completely dry — the Australian dollar, the
  Canadian dollar, the yen, the naira, the lira, all sitting there with
  no market at all. Those are the currencies we're here to make liquid,
  and that needs a venue."
- **Note:** This page is the **segue**, and that's its whole reason to
  exist: it earns the eCLOB by showing the problem in our own data rather
  than asserting it. It also lands the market-data-curation claim, which
  is real work the deck otherwise never mentions. The empty tail is the
  hinge — don't cut the third capture to save space, it is the argument.

#### Page 5 — The eCLOB · ~18s

- **On-slide:** Our design gives order-book transparency with propAMM
  efficiency.
- **Visual:** Left column stacks the two proof captures — the maker's own
  control panel, and the compute-unit pane captioned "Reprice: 47 CU ·
  reshape: 59 CU". Right column is the payoff: that same market rendered
  on the frontend, with the order book, the live trades tape, and a
  filled order together.
- **Spoken:** "Making a market onchain used to be prohibitively expensive
  — gas made continuous quoting impossible, so everything before this was
  a band-aid. We've built order books before, so we built one that fits:
  the eCLOB gives you the transparency of a central limit order book with
  quote updates as cheap as a propAMM. Repricing the whole book costs
  forty-seven compute units and reshaping the ladder fifty-nine, on a
  chain that gives you two hundred thousand per instruction. On the left
  is our own maker running seven markets; on the right is that market on
  the frontend, with the book, the live trades tape, and a filled order.
  We're building this out so anyone can quote onchain with a vault-style
  approach."
- **Note:** The on-slide line states what the design *gives you* rather
  than narrating the history — the "used to be prohibitively expensive"
  framing is strong spoken and too long to read. The right-hand capture
  replaced a strip of four small keyframe thumbnails: one screenshot of
  the whole thing working says more than four stills of it starting up,
  and needs no localnet capture session to produce.

#### Page 6 — How we grow · ~15s

- **On-slide:** Our vaults bootstrap a public FX liquidity flywheel.
- **Visual:** A curve of depth growing, over the flywheel's two ends —
  **Upstream** (AUDD Digital; Loon, who issues CADC) and **Downstream**
  (Altitude, CargoBill), each group alphabetical, **with their logos**.
  Each heading sits over a rule spanning its own two tiles; each tile is
  captioned with the company and what they are to us ("Loon / CADC
  issuer", "Altitude / Banking").
- **Spoken:** "We seed the markets ourselves the way Hyperliquid did —
  our vaults bootstrap each book, and anyone can top them off, so the
  flywheel is public rather than ours alone. The wedge is that long tail
  of currencies: spreads are wide there, and an issuer arriving with no
  depth of their own needs a day-one liquidity partner. And it's a
  two-sided market we're already doing the customer development on.
  Upstream are the stablecoin issuers — AUDD Digital, and Loon, who
  issues CADC — who mint a currency and need it to trade. Downstream is
  the demand: Altitude in banking, CargoBill in supply chain, who need to
  buy FX to settle. Connect the two ends and the depth compounds."
- **Note:** The point of the page is that this is **customer development
  on a two-sided market**, not a partner wish list — so the marks stay.
  An intermediate draft made these text-only tiles on the theory that a
  logo never says what the relationship is; the fix was the caption, not
  removing the logo. Getting the layout to read took several tries, so
  don't undo it: four evenly-spaced tiles look like one row of four, and
  a hairline between them doesn't change that. Boxing each end in a
  filled panel works but the two slabs then dominate the page.
  Heading-plus-rule brackets a pair without that weight. The curve has to
  be wide — a narrow one above a vertical divider read as a chart mounted
  on a stick, which is why there's no divider.

#### Page 7 — Growth roadmap · ~15s

- **On-slide:** Each stage of the roadmap funds the next one. Then three
  beats in time order, spanning the page:
  1. **Now** — DASMAC leads the liquidity. We bootstrap the vaults the
     way Hyperliquid did, and we help issuers get their currency onchain.
     Dropset is the protocol underneath.
  1. **Next** — protocol fees accrue value. As we build a mature market,
     the venue earns on the flow it clears, and the work is getting every
     pair liquid rather than only the largest.
  1. **Later** — derivatives are the expansion. Hedging is an extra
     vertical once spot is nailed, and it's what real market-making
     operations and mature foreign-exchange markets both run on.
- **Visual:** The three beats as a rollout along a rule spanning the full
  page width, not a static list.
- **Note:** Called a **roadmap**, not "commercial viability" — the growth
  story is the frame, and "viability" invites the question of whether it
  is viable. The rollout shape matters: a static list reads as
  speculation, three beats in time order read as a plan. This is also
  where the **DASMAC / Dropset** distinction is made explicit. Name the
  streams in abstracted language, not jargon — no "fee switch".

#### Page 8 — Why the open venue wins · ~12s

- **On-slide:** The people who actually need foreign exchange need an
  open system, and permissioned liquidity is not public.
- **Visual:** No logos and **no competitor names**. A gated panel beside
  an open one — participants either side of a wall, versus every
  participant connected to one book — each captioned with a full
  sentence. The page carries its argument in type and structure.
- **Spoken:** "FX's end consumers need an open system. The honest risk is
  that whoever owns distribution permissions onchain settlement — and
  some of them will try. But permissioned liquidity isn't public: you
  can't make a market unless they let you. Dropset is open, neutral, and
  composable — anyone can quote, anyone can trade, any app can integrate.
  And the venues that *are* public are built for a different customer.
  That's also why we started on Solana: it's the most
  moneyness-conducive environment onchain."
- **Note:** The v1 fail / won't-fail **pair collapsed into this one
  page**, and the asterisk device is retired — it cost a page and the
  payoff didn't carry. Naming competitors on-slide hands them the frame;
  the argument survives without the names, and the fuller counters live
  in the appendix. The characterization of the existing venues' customer
  as day traders is **spoken-only, never on-slide**. This page also
  carries the why-Solana-first beat.

#### Page 9 — Team & close · ~8s

- **On-slide:** Eyebrow "The team", then the sentence "Dropset is built
  by people who have built exchanges" — matching every other page's
  kicker-plus-sentence shape rather than being the one page with a
  different structure. Then one line each: Alex Kahn, Founder — authored
  two exchanges on Aptos, including the Econia order book, which settled
  around \$500M, and wrote the Solana Opcode Guide. Judy Sosa, Operations
  — owns the whole operational stack, working with the banks, stablecoin
  providers, onramps and service providers we build on.
- **Visual:** Both headshots, square and unframed, pulled from the
  marketing site at build time (`remote-assets.json`).
- **Spoken:** "Dropset is built by people who have built exchanges. I
  authored two on Aptos, including the Econia order book, which settled
  around five hundred million in volume, and I wrote the Solana Opcode
  Guide — the playbook for squeezing performance out of Solana programs,
  which is what makes quoting on the eCLOB cost double-digit compute
  units. Judy owns the whole operational stack, and works directly with
  the banks, the stablecoin providers, the onramps and the service
  providers we build on. Dropset — where currency trades onchain."
- **Note:** **State what each person has done; don't argue for why the
  role matters.** An intermediate draft justified the operations split
  ("this is the work that gets an FX venue integrated with the rails…",
  "a dedicated owner rather than a founder's side task") — that reads as
  defending the team, and it framed Judy's work relative to the founder's
  rather than on its own terms. One sentence each, both in the same
  voice. The credential reads "Dragonfly Capital", not "…Partners", with
  the EA role stated plainly. The final spoken line mirrors the title.
  Because this page lingers on screen after the talk, it's the one place
  slightly longer copy is correct — but only slightly.

______________________________________________________________________

## 2. Presentation appendices

Not on slides. Keep the nuance off the deck; put it here and cover it if
you get a call. This is the material to have ready when an investor
grills.

### Team, full

- **Alex — product / exchange design.** Exchange designer; has built
  two onchain exchanges (including an order book) before. Authored
  Econia, the onchain order book on Aptos (~\$500M cleared); co-authored
  emojicoin.fun, a top consumer product on Aptos; and authored the
  Solana Opcode Guide — the playbook for squeezing performance out of
  Solana programs with high-efficiency techniques, which is what drives
  down market-making costs in the eCLOB. Previously cofounded Econia
  Labs.
- **Judy — operations.** Formerly EA at Dragonfly Capital. Owns the
  operational spine end-to-end: opening accounts with the stablecoin
  providers and onramps, plus corporate accounting and service
  providers — the work that gets an FX venue integrated with the
  stablecoin rails. A deliberate split: product and operations each have
  a dedicated owner.

### The competitors, by name — off-slide only

Page 8 names nobody, deliberately. The names belong in conversation,
where they can be answered rather than displayed:

- **The settlement chains (Arc, Tempo) and regulated onchain markets
  (Canton).** Each is chasing onchain settlement and each arrives with
  customers already on it. The answer: they're private or heavily
  permissioned rails. The moment FX needs a *neutral* venue where anyone
  can make a market and anyone can trade, a closed garden can't serve
  it.
- **The existing Solana DEXes (Jupiter, Meteora, Orca, pump.fun,
  Raydium).** They aren't focused on FX, and we're beating them to it.
  It's an innovator's dilemma: the volume today is too small to move a
  giant and big enough for a focused team, and we'll be here for the
  next 10x as payments come onchain. Their customer is a different
  customer — the retail speculator, not the business that needs to
  settle an invoice in another currency.
- **"Why not just be Hyperliquid?"** We borrow Hyperliquid's
  *bootstrapping* playbook (seed the liquidity ourselves), but not its
  verticalized, single-app design. Solana is general-purpose, so
  Dropset is composable: payments providers, merchants, manufacturers,
  and retail can integrate FX settlement directly — DevEx convenience a
  walled venue can't offer.

### Lazy-VC questions to preempt

Have crisp one-liners ready for the questions a VC asks without reading
the deck:

- "What's the market?" → FX, the biggest market on earth (\$9T/day,
  24/5), with no liquid onchain home yet.
- "Who's using it?" → Live on mainnet now (clearing trades via
  aggregators); accelerator partners (Altitude, CargoBill) and
  stablecoin issuers are the first FX demand. We've also spoken with
  providers like CADC and AUDD coming online on Solana who already have
  distribution networks.
- "Why you?" → We've built onchain exchanges before (Econia, ~\$500M);
  this is our domain.
- "Why now?" → Non-US-dollar stablecoins are only just arriving onchain
  (~14 currencies today, euro leading), and payments are following.
- "How do you make money?" → Page 7 is the answer, and the appendix
  detail is that each stage funds the next: liquidity operations now,
  protocol fees as the books thicken, derivatives once there's enough
  depth to hedge against.

______________________________________________________________________

## 3. Formatting / structure rules

### How to read this

- **One page = one slide.** Nine pages, against a ten-page cap (see
  "Format rules").
- Each page gives: the **on-slide line** (what the audience reads), the
  **visual** (the one big image), the **spoken copy** (what the
  presenter says — this is the real script), and a **time** budget.
- Total spoken time targets **~115 seconds**. With the demo videos gone,
  the budget is spread across the pages rather than concentrated in two
  of them.
- Every page carries the same footer: the Dropset wordmark at the left,
  the "Built by DASMAC" credit in the middle, and progress dots at the
  right. It isn't page content — don't budget words or space for it; the
  slide body already reserves room above it.
- Presenter mode is **`⌘⇧P`** (`Ctrl⇧P` off macOS), not a bare `p`.
- Anything nuanced — the competitor rebuttals, the investor grilling,
  the numbers behind a claim — is **not on a slide**. It lives in the
  appendices (section 2) and only comes out if a conversation goes
  there.

### Global rules — v2

These are firm, and they override the older guidance where the two
disagree:

1. **Full sentences everywhere on-slide.** No fragment headlines. A
   reviewer reading the deck without the talk should get the argument.
1. **16:9 aspect ratio**, set explicitly on the deck rather than
   inherited.
1. **Static images only.** No embedded video, no gifs, no player. A
   product beat is an interface screenshot with a claim over it. This
   retires the click-to-play badge and the two recorded demos.
1. **No competitor names or logos on any slide.** Partner logos on the
   growth page stay — that page is customer development on a two-sided
   market, and the marks are the point — but each is captioned with what
   the company is to us.
1. **Solana is never framed as a ceiling.** It's the deliberate start —
   "the most moneyness-conducive environment onchain" — never the
   boundary.
1. **DASMAC is the company, Dropset is the protocol.** The distinction
   has to be legible on the slides: "Built by DASMAC" on the title
   carries it, and the roadmap attributes the bootstrap beat to DASMAC
   explicitly.
1. **No bullet lists** — with exactly one exception, the three facts on
   page 2, which are peers the audience should take at a glance and which
   are still written as full sentences. Everywhere else, prose.

### Brand

The Kargil Studios design system from the DASMAC Figma:

- **Typography.** **Inter** is the primary family, and it carries
  everything in **sentence case** — which is the reason it's primary
  rather than the mono face. **Space Mono** is the mono/tag face, set
  **uppercase and letterspaced**, which is exactly the treatment the
  company banner uses for its own tag. So the deck's kickers and the
  brand art are visibly one system. This supersedes an earlier note
  naming JetBrains Mono. Both are Google fonts loaded through
  `next/font`, so there are no font files to commit.
- **Assets.** `brand-assets/` at the repo root holds the DASMAC and
  Dropset wordmarks, the favicon, and the **DASMAC company banner**
  (`dasmac-banner-wide.png`), all copied into `public/` on the `predev` /
  `prebuild` hooks.

### Export

The deck runs on Spectacle, and it stays there — there is no migration
and no export tooling to build. To produce the accelerator's combined
meta-deck, print the slides through Spectacle's print path (**`⌘⇧R`**)
and drop the resulting images into Google Slides. The only requirement
this puts on the deck is that **every slide prints as a clean 16:9
static page** — nothing clipped, no interactive chrome in the output.

Content that overflows a slide is merely scaled on screen but **silently
clipped in print**, so a layout change means one pass through print mode
before it's done.

### Guidelines — the principles this deck is designed to

Outside advice that reviewers should edit against, so edits land against
the same rules rather than taste.

#### "Design it like a children's book"

Source: <https://x.com/mert/status/1843591496181702766>

> My best advice for making a pitch deck: design it like a children's
> book. i) max 10 pages ii) one big sentence per page iii) one big
> image per page iv) the sentences should tell a story as you flip
> through the pages v) super simple words.
>
> Some other advice: Do not follow a random cookie-cutter template
> about how to structure the deck (i.e. always put team first, or the
> generic problem-solution-market-opportunity thing). Instead,
> understand your best selling points and put those first — if your ARR
> is growing very fast, put that first; if your team has multiple exits
> and understands this domain better than anyone, put that first.
>
> Do not put a market-opportunity slide showing you have a
> trillion-dollar market. You do not — you just don't know how to do
> proper GTM.
>
> The pitch deck is for a VC to scroll through (like Twitter) async in
> a minute or two. Do not put nuanced thoughts and word salad on there
> (they will not read it) — put those in an appendix and cover them if
> you get a call.

Note where v2 **departs** from this: "one big sentence per page" still
holds, but full sentences replaced fragments everywhere, and page 9
carries a line per person because it lingers on screen after the talk. A
reviewer reading only the deck is now a first-class case.

#### Name why it will fail, then answer the counters

Put up the honest risk and don't flinch, then show it's been thought
through — surface the lazy-VC questions and answer them, and be ready to
reply to the counters rather than hoping they don't come up. An investor
respects that the risk was named and met with an answer.

In v1 this was a two-page setup-and-payoff pair with an asterisk gag.
In v2 it is **one page** (page 8): the risk and the answer in the same
breath, with no competitor named on-slide. The steelman versions and the
replies live in the appendix.

### Format rules (distilled from the above)

1. **Max 10 pages.** This deck is 9 — the cap is a ceiling, not a
   target.
1. **One big sentence per page.** No bullet lists, bar the one page-2
   exception noted in the global rules.
1. **One big image per page.** Name the image in the "visual" field. A
   page may show several captures of *the same thing* (the swap flow, the
   currencies table) — that is still one image in the sense that matters,
   because it's one idea.
1. **The sentences tell a story as you flip through.** Read the nine
   on-slide lines top to bottom and they should read as one arc.
1. **Super simple words.**
1. **Lead with the strongest selling point, not a template.** It already
   works, on mainnet, and there's a screenshot — so "live today" is page
   3 and the arc is built around it.
1. **No market-opportunity slide.** FX size appears once, as the shape
   of the gap, never as a trillion-dollar brag.

### Reference — the accelerator's 7-point pitch structure

The Colosseum "basic pitch" framework, from the pitch review in the
fundraise tracker. Not the deck's structure — the children's-book arc
wins for a 2-minute demo — but every point below must be *covered*
somewhere, and this is the checklist the accelerator expects. Mapping to
our pages in brackets.

1. **One-liner.** DASMAC is building Dropset, an onchain Forex platform
   that harnesses Solana for open, efficient exchange of multinational
   currencies at scale. [Pages 1, 9]
1. **Problem / unique insight.** ~14 currencies now live on Solana via
   stablecoins; Solana settlement can support the massive FX market
   *composably* — DevEx convenience for payments providers, merchants,
   manufacturers, and retail — because Solana is general-purpose, not
   verticalized like Hyperliquid. [Pages 2, 4, 8; appendix]
1. **Solution / product.** Dropset routes existing onchain liquidity
   through aggregators and adds a novel eCLOB to bootstrap new markets
   with inexpensive quote updates that accelerate market-maker
   onboarding. [Pages 3, 5]
1. **Traction.** Dropset.io is live and clearing trades on mainnet
   (today via aggregators), and curates the market data for every
   currency onchain. [Pages 3, 4]
1. **Why the market is massive.** FX is >\$9T/day and 24/5; Solana as
   intermediary gives atomic settlement and faster on/off-ramps. \[Page
   2\]
1. **Why now.** The non-US stablecoin market has only just started to
   expand — EUR stablecoins drive most volume, more currencies going
   live (14 on Solana). [Pages 2, 4]
1. **Business model.** Liquidity operations now, protocol fees with an
   illiquid-pair premium next, derivatives after that. [Page 7]
1. **Founders' bio.** Exchange-design background — authored the Econia
   order book (~\$500M on Aptos) and the Solana Opcode Guide — with a
   dedicated operations owner on banking and accounting. Full detail on
   Page 9 and in the appendix (kept there to stay DRY). \[Page 9;
   appendix\]
