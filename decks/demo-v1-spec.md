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
revision is the consensus of a planning session over the round-1
reviewer feedback. What changed, at a glance:

- Full sentences on every slide, in place of fragment headlines.
- **Static images only.** The two recorded demos and the click-to-play
  badge are gone; product beats are interface screenshots carrying a
  full-sentence claim.
- The eCLOB page is **inverted to why-first** — cost was the barrier,
  and the compute-unit numbers are the answer, not the opener.
- **How we grow moved up** to follow the eCLOB directly.
- A **new commercial-viability page**, written as a rollout in time.
- The "why this will fail" / "why it won't" pair **collapsed into one
  page**, and the asterisk device is retired.
- **No competitor names or logos anywhere**, and partner tiles are
  text-only.

______________________________________________________________________

## 1. Slide contents

### The 2-minute narrative (continuous read)

The through-line, so the story reads as one piece before it's cut into
pages:

> Dropset is where currency trades onchain, built by DASMAC. Foreign
> exchange is the biggest market on earth — over nine trillion dollars a
> day — but it trades only 24/5, its liquidity is fragmented across
> obfuscated over-the-counter desks, and less than ten percent of the
> world's currencies are even available on Solana today. We think every
> currency should be connectable to every other one, and we're building
> the place that happens. It already works: Dropset is live on mainnet,
> clearing real trades — settlement is atomic, the ramps are near
> instant, and the venue never closes. Solana is the start, not the end:
> it's the most moneyness-conducive environment onchain. Running a real
> market onchain used to be prohibitively expensive — gas made
> continuous quoting impossible, and everything before this was a
> band-aid. So we built the eCLOB, where repricing the whole book costs
> forty-seven compute units and reshaping the ladder fifty-nine. We've
> built order books before. That's what lets our vaults bootstrap a
> public FX liquidity flywheel: the wedge is the long tail of currencies
> as they come onchain, where spreads are wide and issuers arrive with
> no depth of their own. It's a real business at each stage — today
> DASMAC runs the liquidity operations, next the exchange takes protocol
> fees with a premium on the illiquid pairs, and after that derivatives,
> hedging in particular, which deepens the market making and serves
> treasury flows as payments come onchain. FX's end consumers need an
> open system. The honest risk is that whoever owns distribution
> permissions onchain settlement — but permissioned liquidity isn't
> public, and Dropset is open, neutral, and composable. This is DASMAC:
> two onchain exchanges built, the Econia order book on Aptos authored,
> the Solana Opcode Guide written, and operations owned end-to-end.
> Dropset — where currency trades onchain.

### Page-by-page

#### Page 1 — Title · ~5s

- **On-slide:** Where currency trades onchain. Beneath it: Built by
  DASMAC.
- **Visual:** The Dropset wordmark on the dark theme, with the DASMAC
  brand motifs. Two **banner slots** sit here as blank placeholder
  frames — the DASMAC company banner (the one with the mountains) and a
  Dropset protocol banner counterpart — presented as a coherent pair so
  the lockup can be judged before the real art exists.
- **Spoken:** "Dropset is where currency trades onchain."
- **Note:** "Built by DASMAC", not "courtesy of" — the credit is
  authorship, and it's what carries the company/protocol distinction
  from the first frame. Solana is **not** mentioned on this page; the
  old "Forex on Solana" line implied a boundary the deck no longer
  wants. There is **no separate closing slide** — the deck ends on the
  team page, which stays up after the talk.

#### Page 2 — The gap · ~15s

- **On-slide:** Foreign exchange is the biggest market on earth, and
  less than 10% of the world's currencies are available on Solana
  today.
- **Visual:** The Dropset frontend's currencies count — 14 of 162
  represented, 148 not yet listed — captioned with the page it's from,
  `dropset.io/currencies`, so the count is verifiable.
- **Spoken:** "Foreign exchange is over nine trillion dollars a day.
  But it trades only 24/5, its liquidity is fragmented across
  obfuscated over-the-counter desks, and less than ten percent of the
  world's currencies are even available on Solana today — fourteen out
  of a hundred and sixty-two, and that count is live on the site. Every
  currency should be connectable to every other one, and that's what
  we're building."
- **Note:** Frame it as **gap plus upside**, never as a market-size
  slide. The every-currency vision beat starts here, and it is worded as
  **connection, not issuance** — Dropset does not issue currencies;
  issuers create them and Dropset is where they trade. The ~\$9T/day
  figure needs no citation (it isn't disputed at pitch-deck level), but
  the currency count keeps its attribution because it's ours and it's
  checkable. **Do not invent a Solana volume-share percentage** —
  currency count and global volume only.

#### Page 3 — Live today · ~15s

- **On-slide:** Dropset is settling real trades on mainnet today.
- **Visual:** Interface screenshots of the mainnet swap flow — the globe,
  the route, the settled trade. Built as clearly labeled placeholder
  frames until the real captures are dropped in.
- **Spoken:** "This already works. Dropset is live on mainnet today,
  clearing real trades: settlement is atomic, the ramps are near
  instant, and the venue never closes. Solana is the start, not the end
  — it's the most moneyness-conducive environment onchain."
- **Note:** The *why onchain matters* beat lives here, spoken — atomic
  settlement, near-instant ramps, always-on. Keep the claim exact: today
  we clear by routing through aggregators, and the eCLOB on the next
  page is how we **bootstrap** the markets that have no liquidity yet.
  Don't assert "most liquid". Nothing on this page depends on a live
  network — the screenshots are static.

#### Page 4 — The eCLOB · ~20s

- **On-slide:** Running a real market onchain used to be prohibitively
  expensive, so we made quoting nearly free.
- **Visual:** The market-maker TUI and the compute-unit pane, captioned
  "Reprice: 47 CU · reshape: 59 CU", plus a short static keyframe strip
  of a market coming alive — empty book, makers on, depth, a fill.
- **Spoken:** "Running a real market onchain used to be prohibitively
  expensive. Gas made continuous quoting impossible, so everything
  before this was a band-aid. We've built order books before, so we
  built one that fits: on the eCLOB, repricing the whole book costs
  forty-seven compute units and reshaping the ladder fifty-nine — on a
  chain that gives you two hundred thousand per instruction. That's what
  lets us bootstrap a brand-new market and onboard makers fast."
- **Note:** **Why-first, inverted from v1.** The eCLOB is *how*, not
  *why*, so the page opens on the barrier and the compute-unit numbers
  land as the answer. The "we've built order books before" clause is
  deliberate: the why-us answer should land while presenting, not only
  on the team slide at the end. The bootstrap-from-empty beat is a
  static keyframe strip, not a video.

#### Page 5 — How we grow · ~20s

- **On-slide:** Our vaults bootstrap a public FX liquidity flywheel.
- **Visual:** A curve of depth growing, over the flywheel's two ends —
  **Upstream** (AUDD Digital; Loon, who issues CADC) and **Downstream**
  (Altitude, CargoBill), each group alphabetical. **Text-only tiles, no
  logos.** Each tile is captioned with a full sentence naming what the
  company is to us and that we've spoken with them about sourcing
  liquidity.
- **Spoken:** "We seed the markets ourselves the way Hyperliquid did —
  our vaults bootstrap each book, and anyone can top them off, so the
  flywheel is public rather than ours alone. The wedge is the long tail
  of currencies as they come onchain: spreads are wide there, and an
  issuer arriving with no depth needs a day-one liquidity partner. It
  scales from today's basket toward full G7 coverage. And it has two
  ends we've already talked to — upstream, the issuers who mint a
  currency and need it to trade; downstream, the payments companies who
  need to buy FX to settle. Connect the two and the depth compounds."
- **Note:** **Moved up** to follow the eCLOB directly, so the growth
  story lands while the venue is still fresh. This page absorbs the
  accelerator's GTM ask — long-tail wedge, wide spreads, day-one
  liquidity partner, a path toward G7 coverage. The full-sentence
  captions are the fix for the round-1 "why these companies?" confusion:
  a bare logo grid never said what the relationship was. Logo outreach
  is dropped for now.

#### Page 6 — Commercial viability · ~15s

- **On-slide:** Three beats in time order — **Now:** DASMAC bootstraps
  the vaults, and liquidity operations are the company's business.
  **Next:** the exchange takes protocol fees, with a natural premium on
  illiquid pairs. **Later:** derivatives, hedging in particular.
- **Visual:** The three beats as a rollout along a timeline, not a
  static list.
- **Spoken:** "This is a business at every stage. Today DASMAC — the
  company — bootstraps the vaults the way Hyperliquid did, so the
  liquidity operations are ours and Dropset is the protocol they run on.
  Next, the exchange takes fees, with a natural premium on the illiquid
  pairs nobody else quotes. After that, derivatives — hedging in
  particular, the ability to go short, which itself deepens the market
  making on the venue and serves business treasury flows as payments
  come onchain."
- **Note:** New page in v2 — round-1 feedback wanted the revenue
  question answered rather than implied. Name the streams in
  **abstracted language**, not dodged and not in jargon: no "fee
  switch". The rollout shape matters — a static list reads as
  speculation, three beats in time order read as a plan. This page is
  also where the **DevCo / protocol** distinction is made explicit.

#### Page 7 — Why the open venue wins · ~15s

- **On-slide:** The people who actually need foreign exchange need an
  open system, and permissioned liquidity is not public.
- **Visual:** No logos and **no competitor names** — not the settlement
  chains, not the Solana DEXes. The page carries its argument in type.
- **Spoken:** "FX's end consumers need an open system. The honest risk
  is that whoever owns distribution permissions onchain settlement — and
  some of them will try. But permissioned liquidity isn't public: you
  can't make a market unless they let you. Dropset is open, neutral, and
  composable — anyone can quote, anyone can trade, any app can
  integrate. And the venues that *are* public are built for a different
  customer. That's also why we started on Solana: it's the most
  moneyness-conducive environment onchain."
- **Note:** The v1 fail / won't-fail **pair collapsed into this one
  page**, and the asterisk device is retired — it cost a page and the
  payoff didn't carry. Round-1 feedback was firm that naming
  competitors on-slide hands them the frame; the argument survives
  without the names, and the fuller counters live in the appendix.
  The "not Singaporean day traders" characterization of the existing
  venues' customer is **spoken-only, never on-slide**. This page also
  carries the why-Solana-first beat.

#### Page 8 — Team & close · ~10s

- **On-slide:** This is the one long-copy page — full sentences people
  can read in depth, because it stays up after the talk ends. Alex
  Kahn: has built two onchain exchanges, authored the Econia order book
  on Aptos (~\$500M cleared), and wrote the Solana Opcode Guide. Judy
  Sosa: owns operations end-to-end — banking, stablecoin providers,
  onramps, and accounting — prev. EA, Dragonfly Capital.
- **Visual:** Both headshots, square and unframed, pulled from the
  marketing site at build time (`remote-assets.json`).
- **Spoken:** "This is DASMAC. I've built two onchain exchanges,
  including an order book — I authored Econia on Aptos, which cleared
  around five hundred million in volume, and wrote the Solana Opcode
  Guide, the playbook for squeezing performance out of Solana programs.
  Judy owns operations end-to-end: banking, the stablecoin providers,
  onramps, and accounting. Dropset — where currency trades onchain."
- **Note:** **CV flex, not modesty** — round-1 feedback was that the
  credentials were undersold. Frame it as DASMAC the company building
  this, not two engineers with a prototype. The credential reads
  "Dragonfly Capital", not "…Partners", with the EA role stated plainly.
  The final spoken line mirrors the title, closing the loop. Because
  this page lingers, it's the one place long copy is correct.

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

Page 7 names nobody, deliberately. The names belong in conversation,
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
- "How do you make money?" → Page 6 is the answer, and the appendix
  detail is that each stage funds the next: liquidity operations now,
  protocol fees as the books thicken, derivatives once there's enough
  depth to hedge against.

______________________________________________________________________

## 3. Formatting / structure rules

### How to read this

- **One page = one slide.** Eight pages, against a ten-page cap (see
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
   product beat is an interface screenshot with a full-sentence claim
   over it ("Dropset is settling real trades on mainnet today"). This
   retires the click-to-play badge and the two recorded demos
   completely.
1. **No logo rows for competitors or threats**, and no competitor names
   on any slide. Partner tiles on the growth page stay, and each is
   captioned with a full sentence saying what that company is to us.
1. **Solana is never framed as a ceiling.** It's the deliberate start —
   "the most moneyness-conducive environment onchain" — never the
   boundary.
1. **DASMAC is the company, Dropset is the protocol.** The distinction
   has to be legible on the slides: "Built by DASMAC" on the title
   carries it, and the commercial-viability rollout attributes the
   bootstrap beat to DASMAC explicitly.

### Brand

The Kargil Studios design system from the DASMAC Figma:

- **Typography.** **Inter** is the primary family; **Space Mono** is the
  mono/tag face. Space Mono is what the product website types in, and
  consistency with the site wins — this supersedes an earlier note
  naming JetBrains Mono. Both are Google fonts, loaded through
  `next/font`, so there are no font files to commit.
- **Assets.** `brand-assets/` at the repo root holds the DASMAC and
  Dropset wordmarks and the favicon, copied into `public/` on the
  `predev` / `prebuild` hooks. The DASMAC company banner (the Twitter
  banner, the one with the mountains) is **not** in the repo — the deck
  ships a blank placeholder frame for it, paired with one for a Dropset
  protocol banner.

### Export

The deck runs on Spectacle, and it stays there — there is no migration
and no export tooling to build. To produce the accelerator's combined
meta-deck, print the slides through Spectacle's print path (**`⌘⇧R`**)
and drop the resulting images into Google Slides. The only requirement
this puts on the deck is that **every slide prints as a clean 16:9
static page** — nothing clipped, no interactive chrome in the output.

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
holds for pages 1–7, but full sentences replaced fragments everywhere,
and page 8 is deliberately long-copy because it lingers on screen after
the talk. A reviewer reading only the deck is now a first-class case.

#### Name why it will fail, then answer the counters

Put up the honest risk and don't flinch, then show it's been thought
through — surface the lazy-VC questions and answer them, and be ready to
reply to the counters rather than hoping they don't come up. An investor
respects that the risk was named and met with an answer.

In v1 this was a two-page setup-and-payoff pair with an asterisk gag.
In v2 it is **one page** (page 7): the risk and the answer in the same
breath, with no competitor named on-slide. The steelman versions and the
replies live in the appendix.

### Format rules (distilled from the above)

1. **Max 10 pages.** This deck is 8 — the cap is a ceiling, not a
   target. v2 added a commercial-viability page and removed one by
   collapsing the fail / won't-fail pair, so the count held.
1. **One big sentence per page** (pages 1–7). No bullet lists.
1. **One big image per page.** Name the image in the "visual" field.
1. **The sentences tell a story as you flip through.** Read the eight
   on-slide lines top to bottom and they should read as one arc.
1. **Super simple words.**
1. **Lead with the strongest selling point, not a template.** It
   already works, on mainnet, and there's a screenshot — so "live
   today" is page 3 and the arc is built around it.
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
   currencies at scale. [Pages 1, 8]
1. **Problem / unique insight.** ~14 currencies now live on Solana via
   stablecoins; Solana settlement can support the massive FX market
   *composably* — DevEx convenience for payments providers, merchants,
   manufacturers, and retail — because Solana is general-purpose, not
   verticalized like Hyperliquid. [Pages 2, 7; appendix]
1. **Solution / product.** Dropset routes existing onchain liquidity
   through aggregators and adds a novel eCLOB to bootstrap new markets
   with inexpensive quote updates that accelerate market-maker
   onboarding. [Pages 3, 4]
1. **Traction.** Dropset.io is live and clearing trades on mainnet
   (today via aggregators), with more market-making and exchange
   components built in the open. [Page 3]
1. **Why the market is massive.** FX is >\$9T/day and 24/5; Solana as
   intermediary gives atomic settlement and faster on/off-ramps. \[Page
   2\]
1. **Why now.** The non-US stablecoin market has only just started to
   expand — EUR stablecoins drive most volume, more currencies going
   live (14 on Solana). [Page 2]
1. **Business model.** Liquidity operations now, protocol fees with an
   illiquid-pair premium next, derivatives after that. [Page 6]
1. **Founders' bio.** Exchange-design background — authored the Econia
   order book (~\$500M on Aptos) and the Solana Opcode Guide — with a
   dedicated operations owner on banking and accounting. Full detail on
   Page 8 and in the appendix (kept there to stay DRY).
   [Page 8; appendix]
