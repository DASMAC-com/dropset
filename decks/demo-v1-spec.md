<!-- cspell:word Aptos -->
<!-- cspell:word AUDD -->
<!-- cspell:word CADC -->
<!-- cspell:word Cargobill -->
<!-- cspell:word Dragonfly -->
<!-- cspell:word Econia -->
<!-- cspell:word emojicoin -->
<!-- cspell:word Hibachi -->
<!-- cspell:word onramps -->

# Demo-day pitch spec — `demo-v1`

The **copy** for the ~2-minute demo-day pitch, written to be reviewed
and edited *before* it's turned into slides. This is the script and
page plan; the built deck lives at `app/demo-v1/` and should follow
this doc, not the other way around. Drop it into Google Docs, get
edits from others, then reconcile the deck to match.

Sections are ordered for Google Docs toggles: **1. Slide contents**
(the actual copy) first, **2. Presentation appendices** (the off-slide
Q&A material) next, and **3. Formatting / structure rules** (how to
read this, the design principles, the reference structure) last.

---

## 1. Slide contents

### The 2-minute narrative (continuous read)

The through-line, so the story reads as one piece before it's cut into
pages:

> Dropset is Forex on Solana. The biggest market in the world — over
> nine trillion dollars a day, trading 24/5 — barely exists onchain:
> only about 14 of the world's currencies live on Solana so far. So we
> built the eCLOB: the depth of an order book, with quote updates as
> cheap as a propAMM. Here's the stack — this is the market-maker's
> control panel, and here's a swap clearing on the frontend. Now watch
> a brand-new market come alive: the book starts empty, I turn the
> makers on, and real depth fills in within seconds. And just like
> that, my laptop is quoting FX on Solana technology — Dropset already clears
> trades on mainnet today by routing through aggregators, and the eCLOB
> is how we bootstrap the markets that don't exist yet. Why won't this
> work? Arc, Tempo, Hibachi, and Canton are all chasing onchain
> settlement — but they're private, permissioned, or walled gardens,
> and big apps like Jupiter aren't focused on FX. Dropset is the open,
> neutral, composable venue anyone can quote on and anyone can trade
> against, and we're beating them to it. We bootstrap the liquidity
> ourselves the way Hyperliquid did — through a vault others can top off
> — and help stablecoin issuers land their first trades onchain;
> Colosseum partners like Altitude and Cargobill already need FX. The
> team has built exchanges before — I authored the Econia order book and
> the Solana Opcode Guide, and Judy owns the operations that get us
> integrated with stablecoin rails. Dropset — Forex on Solana.

### Page-by-page

#### Page 1 — Title · ~5s

- **On-slide:** Forex on Solana.
- **Visual:** Dropset wordmark, centered, on the dark theme.
- **Spoken:** "Dropset is onchain Forex on Solana — providing open and
  efficient exchange of the world's currencies at scale."

#### Page 2 — The gap · ~12s

- **On-slide:** The biggest market on earth barely exists onchain.
- **Visual:** The Dropset frontend's currencies page — e.g. "14 of 159
  currencies listed," the rest greyed out. (One image — not a stat
  table.)
- **Spoken:** "Foreign exchange is over nine trillion dollars a day,
  and it trades 24/5 — but onchain it has no liquid home. Only about 14
  of the world's currencies are represented on Solana today, with the
  euro driving most of the volume. Settle FX through Solana and you get
  atomic settlement and near-instant on- and off-ramps."
- **Note:** This is the one place FX size is mentioned. Frame it as the
  *gap*, said out loud, never as a market-size slide (per Mert).

#### Page 3 — The eCLOB · ~15s

- **On-slide:** So we built the eCLOB: order-book depth, propAMM-cheap
  quotes.
- **Visual:** A clean order-book ladder (the TUI/frontend book view).
- **Spoken:** "Our edge is a new exchange design — the eCLOB. You get
  the liquidity guarantees of a central limit order book, but quote
  updates as cheap as a propAMM. That lets us bootstrap brand-new
  markets and onboard market makers far faster."
- **Note:** Two ways we source liquidity — worth having straight: where
  a currency already has some onchain liquidity, we route through
  **aggregators**; the **eCLOB** is how we **bootstrap the new markets**
  where there's none yet. The demo shows the second.

#### Page 4 — The stack: maker panel + a swap [DEMO · localnet] · ~25s

- **On-slide:** Here's the market-maker's control panel — and a swap
  clearing.
- **Visual:** The maker TUI / control panel, then a swap on the
  frontend. Slide shows the command **`make demo`**.
- **Spoken:** "Let me show you the stack. This is our market-maker
  control panel — the TUI a maker uses to quote a book. And here's the
  user side: I run a swap on the frontend and it clears. [run
  `make demo`]"
- **Demo ops:** This whole demo runs on **localnet**, driven by
  `make demo`. If anything fails live, fall back to the recorded video.

#### Page 5 — A market comes alive [DEMO · localnet] · ~25s

- **On-slide:** Empty book, makers on — real depth in seconds.
- **Visual:** Split view — the maker TUI on one side, the frontend
  order book on the other, book filling from empty. Slide shows
  **`make demo`**.
- **Spoken:** "Now watch a brand-new market come alive. The book starts
  empty — I turn the maker bots on, and top-of-book fills in live. Then
  I trade against real eCLOB depth and it fills the size. This is the
  market-maker's view; the frontend is the user's view."
- **Note:** The point of this beat: we have a working market-maker
  control panel and frontend, and this is exactly how we bootstrap
  liquidity. The *next* step is wiring it up on mainnet (Page 9).
  Optional flourish if time allows: from the TUI, reshape the ladder or
  reprice the whole book in a single instruction.

#### Page 6 — Traction · ~8s

- **On-slide:** And just like that, my laptop is quoting FX on
  Solana.
- **Visual:** The presenter's actual laptop, or the multi-market TUI
  showing several books quoting at once.
- **Spoken:** "And just like that, my laptop is quoting FX on
  Solana. This isn't only a demo — Dropset already clears trades on
  mainnet today by routing through aggregators, and what you just saw is
  how we bootstrap the brand-new markets with the eCLOB."
- **Note:** The live demo is localnet. The mainnet traction is real but
  specific: today Dropset clears trades by routing existing liquidity
  through **aggregators**; the eCLOB + maker demo is how we **bootstrap**
  liquidity where none exists. Don't assert "most liquid right now" —
  that isn't true during a localnet demo.

#### Page 7 — Why this will fail · ~12s

- **On-slide:** Why won't this work? Arc, Tempo, Hibachi, Canton.
- **Visual:** The competitor logos as a wall closing in.
- **Spoken:** "The honest risk: everyone wants onchain settlement. Arc,
  Tempo, and Hibachi are building payment-and-settlement rails, and
  Canton is doing regulated onchain markets. Any of them could decide
  FX is theirs."
- **Note:** This slide is deliberate — an investor respects that we
  named the threat first (Clay's + Alex's advice). The rebuttal is the
  very next page. Fuller framing in the appendix.

#### Page 8 — Why it will work · ~8s

- **On-slide:** They're private or walled — and the big apps aren't
  focused on FX.
- **Visual:** A single open door vs. a row of locked ones.
- **Spoken:** "But those are private, permissioned, or walled gardens.
  And big Solana apps like Jupiter aren't focused on FX — it's a smaller
  market today, so it's a classic innovator's dilemma: only a small,
  focused team goes after it now. Dropset is the open, neutral,
  composable venue — anyone can quote, anyone can trade, any app can
  integrate — and we're beating everyone to it."
- **Note:** Two rebuttals in one breath — (1) the closed-garden
  competitors, (2) the unfocused incumbents (an app like Jupiter /
  innovator's dilemma, smaller market now). The composability angle
  (general-purpose Solana vs. a verticalized venue like Hyperliquid) is
  expanded in the appendix.

#### Page 9 — How we grow · ~6s

- **On-slide:** We bootstrap the liquidity ourselves — like Hyperliquid.
- **Visual:** A curve of depth growing; logos of Colosseum partners
  (Altitude, Cargobill).
- **Spoken:** "We seed the markets ourselves the way Hyperliquid did —
  through a vault others can top off with inventory — and we help
  stablecoin issuers land their first real trades on mainnet. Colosseum
  partners like Altitude and Cargobill already need to source FX onchain
  — that's our first demand."

#### Page 10 — Team & close · ~4s

- **On-slide:** Built by people who've built exchanges.
- **Visual:** Alex + Judy, then the Dropset wordmark.
- **Spoken:** "I've built two onchain exchanges already, including an
  order book — I authored Econia on Aptos, which cleared around five
  hundred million in volume, and wrote the Solana Opcode Guide, the
  playbook for squeezing performance out of Solana programs. Judy owns
  operations end-to-end — banking, the stablecoin providers, onramps,
  and accounting. Dropset — Forex on Solana."

---

## 2. Presentation appendices

Not on slides. Mert: keep the nuance off the deck; put it here and
cover it if you get a call. This is the material to have ready when an
investor grills.

### Team, full

- **Alex — product / exchange design.** Exchange designer; has built
  two onchain exchanges (including an order book) before. Authored
  Econia, the onchain order book on Aptos (~$500M cleared); co-authored
  emojicoin.fun, a top consumer product on Aptos; and authored the
  Solana Opcode Guide — the playbook for squeezing performance out of
  Solana programs with high-efficiency techniques, which is what drives
  down market-making costs in the eCLOB. Previously cofounded Econia
  Labs.
- **Judy — operations.** Formerly EA at Dragonfly. Owns the operational
  spine end-to-end: opening accounts with the stablecoin providers and
  onramps, plus corporate accounting and service providers — the work
  that gets an FX venue integrated with the stablecoin rails. A
  deliberate split: product and operations each have a dedicated owner.

### Why this will fail — the steelman, and the answer

- **"Arc / Stripe / Tempo are good enough; open access doesn't
  matter."** — Answer: they're private or heavily permissioned rails.
  The moment FX needs a *neutral* venue where anyone can make a market
  and anyone can trade, a closed garden can't serve it. We're building
  the open venue and getting there first.
- **"Hibachi / Canton are direct competitors."** — Hibachi and the
  Arc/Tempo camp are chasing settlement; Canton is regulated onchain
  markets. But they're permissioned or walled — a different animal from
  an open, composable FX book.
- **"Why wouldn't Jupiter or a big app just do this?"** — They aren't
  focused on FX, and we're beating them to it. It's an innovator's
  dilemma: an open, FX-specialized venue only makes sense for a small,
  focused team to chase right now — the volume (a few million a day
  today) is too small to move a giant, and we'll be here for the next
  10x as payments come onchain.
- **"Why not just be Hyperliquid?"** — We borrow Hyperliquid's
  *bootstrapping* playbook (seed the liquidity ourselves), but not its
  verticalized, single-app design. Solana is general-purpose, so
  Dropset is composable: payments providers, merchants, manufacturers,
  and retail can integrate FX settlement directly — DevEx convenience a
  walled venue can't offer.
- **"Show me you've thought about every angle."** — The point of this
  section: an investor wants to see the failure modes named and
  answered, not hidden.

### Lazy-VC questions to pre-empt

Have crisp one-liners ready for the questions a VC asks without reading
the deck:

- "What's the market?" → FX, the biggest market on earth ($9T/day,
  24/5), with no liquid onchain home yet.
- "Who's using it?" → Live on mainnet now (clearing trades via
  aggregators); Colosseum partners (Altitude, Cargobill) and stablecoin
  issuers are the first FX demand. We've also spoken with providers like
  CADC and AUDD coming online on Solana who already have distribution
  networks.
- "Why you?" → We've built onchain exchanges before (Econia, ~$500M);
  this is our domain.
- "Why now?" → Non-US-dollar stablecoins are only just arriving onchain
  (~14 currencies today, euro leading), and payments are following.

---

## 3. Formatting / structure rules

### How to read this

- **One page = one slide.** Ten pages, hard cap (see "Format rules").
- Each page gives: the **on-slide line** (the single big sentence the
  audience reads), the **visual** (the one big image), the **spoken
  copy** (what the presenter says — this is the real script), and a
  **time** budget.
- Total spoken time targets **~120 seconds**. The two live-demo beats
  eat ~50s of that, so every other page has to be fast.
- Anything nuanced — the competitor rebuttals, the investor grilling,
  the numbers behind a claim — is **not on a slide**. It lives in the
  appendices (section 2) and only comes out if a conversation goes
  there.

### Guidelines — the principles this deck is designed to

Two pieces of outside advice are the priority for this deck. They're
quoted here so reviewers edit against the same rules, not just taste.

#### Mert — "design it like a children's book"

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

#### Clay — name why it will fail, then answer the counters

Put up a **"why this will fail"** beat and don't flinch: say the honest
version an investor is already thinking ("Arc, Stripe, and Tempo are
good enough; open access isn't actually important"). Then show you've
thought it through — **surface the lazy-VC questions and answer them**,
and be ready to **reply to the counters** rather than hoping they don't
come up. An investor respects that the risk was named first and met
with an answer. The honest threat is Page 7; the answer is Page 8; the
fuller counters-and-replies live in the appendices.

### Format rules (distilled from the above)

Design it like a children's book, because a demo-day audience scrolls
it like Twitter — they will not read a word salad.

1. **Max 10 pages.** This deck is exactly 10.
1. **One big sentence per page.** No bullet lists. Where the current
   built deck uses three-bullet lists (pages 2, 3, 6), the deck should
   collapse each to a single line to match this spec.
1. **One big image per page.** Name the image in the "visual" field.
1. **The sentences tell a story as you flip through.** Read the ten
   on-slide lines top to bottom and they should read as one arc.
1. **Super simple words.**
1. **Lead with the strongest selling point, not a template.** The
   strongest point here is *it works and I can show it running in front
   of you* — so the demo comes early and the arc is built around it. No
   generic problem → solution → market-size structure.
1. **No market-opportunity slide.** We do not put up a "$9T TAM"
   slide. FX size appears once, as the *shape of the gap* ("the
   biggest market on earth barely exists onchain"), never as a
   trillion-dollar brag.

### Reference — the accelerator's 7-point pitch structure

The Colosseum "basic pitch" framework (via Nate), from the pitch review
in the fundraise tracker. Not the deck's structure — Mert's
children's-book arc wins for a 2-minute demo — but every point below
must be *covered* somewhere, and this is the checklist the accelerator
expects. Mapping to our pages in brackets.

1. **One-liner.** DASMAC is building Dropset, an onchain Forex platform
   that harnesses Solana for open, efficient exchange of multinational
   currencies at scale. [Pages 1, 10]
1. **Problem / unique insight.** ~14 currencies now live on Solana via
   stablecoins; Solana settlement can support the massive FX market
   *composably* — DevEx convenience for payments providers, merchants,
   manufacturers, and retail — because Solana is general-purpose, not
   verticalized like Hyperliquid. [Pages 2, 8; appendix]
1. **Solution / product.** Dropset routes existing onchain liquidity
   through aggregators and adds a novel eCLOB to bootstrap new markets
   with inexpensive quote updates that accelerate market-maker
   onboarding. [Pages 3, 4, 5]
1. **Traction.** Dropset.io is live and clearing trades on mainnet
   (today via aggregators), with more market-making and exchange
   components built in the open. [Page 6]
1. **Why the market is massive.** FX is >$9T/day and 24/5; Solana as
   intermediary gives atomic settlement and faster on/off-ramps. [Page
   2]
1. **Why now.** The non-US stablecoin market has only just started to
   expand — EUR stablecoins drive most volume, more currencies going
   live (14 on Solana). [Page 2]
1. **Founders' bio.** Exchange-design background — authored the Econia
   order book (~$500M on Aptos) and the Solana Opcode Guide — with a
   dedicated operations owner (Judy) on banking and accounting. Full
   detail on Page 10 and in the appendix (kept there to stay DRY).
   [Page 10; appendix]
