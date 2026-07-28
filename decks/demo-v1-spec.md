<!-- cspell:word cofounded -->

<!-- cspell:word composably -->

<!-- cspell:word Dragonfly -->

<!-- cspell:word emojicoin -->

<!-- cspell:word fundraise -->

<!-- cspell:word Mert -->

<!-- cspell:word steelman -->

<!-- cspell:word verticalized -->

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

______________________________________________________________________

## 1. Slide contents

### The 2-minute narrative (continuous read)

The through-line, so the story reads as one piece before it's cut into
pages:

> Dropset is Forex on Solana. The biggest market in the world — over
> nine trillion dollars a day, trading 24/5 — barely exists onchain:
> only about 14 of the world's currencies live on Solana so far. But
> Dropset is changing that, and it already works: we're live on mainnet
> today, clearing real trades by routing FX through aggregators — pick a
> currency and the swap settles. And we're just getting started, because
> the markets that don't exist yet need a venue. So we built the eCLOB:
> the depth of an order book, with quote updates as cheap as a propAMM —
> a maker repricing the whole book costs forty-seven compute units. Watch a
> brand-new market come alive: the book starts empty, the makers come
> on, and real depth fills in within seconds. So why will this fail?
> Permissioned distribution: Arc, Tempo, and Canton are all chasing
> onchain settlement, and each arrives with the customers already on it.
> But their liquidity isn't public — you can't make a market unless they
> let you — and the big Solana DEXes, Jupiter and Orca and Raydium, focus
> on SOL and memes. Dropset is the open, neutral, composable venue anyone
> can quote on and anyone can trade against, and we're beating them to
> it. We bootstrap a public liquidity flywheel — we seed the vaults,
> anyone can top them off — and it has two ends we've already talked to:
> upstream the issuers like CADC and AUDD who need their currency to
> trade, downstream payments companies like Altitude and CargoBill who
> need to buy FX to settle. The team has built exchanges before — I
> cofounded Econia Labs and authored the Econia order book and the Solana
> Opcode Guide, and Judy — previously at Dragonfly Capital Partners —
> owns the operations that get us integrated with stablecoin rails.
> Dropset — Forex on Solana.

### Page-by-page

#### Page 1 — Title · ~5s

- **On-slide:** Forex on Solana.
- **Visual:** Dropset wordmark, centered, on the dark theme.
- **Spoken:** "Dropset is onchain Forex on Solana — providing open and
  efficient exchange of the world's currencies at scale."

#### Page 2 — The gap · ~12s

- **On-slide:** The biggest market on earth barely exists onchain.
- **Visual:** The Dropset frontend's currencies count — "14 of 162
  currencies represented on Solana, 148 not yet listed" — captioned with
  the page it's from, `dropset.io/currencies`, so the audience can go
  check the number themselves. (One image — not a stat table.)
- **Spoken:** "Foreign exchange is over nine trillion dollars a day,
  and it trades 24/5 — but onchain it has no liquid home. Only about 14
  of the world's currencies are represented on Solana today, with the
  euro driving most of the volume — that count is live on
  dropset.io/currencies, where this is from. Settle FX through Solana
  and you get atomic settlement and near-instant on- and off-ramps."
- **Note:** This is the one place FX size is mentioned. Frame it as the
  *gap*, said out loud, never as a market-size slide (per Mert).

#### Page 3 — Live on mainnet [DEMO VIDEO · mainnet] · ~25s

- **On-slide:** But Dropset is changing this.
- **Visual:** The globe, currencies pinned to the countries that issue
  them, with a route drawn across it. Badge **demo video · mainnet**,
  set beside the globe rather than under it — the capture is tall, and a
  badge below it collides with the footer.
- **Spoken:** "But Dropset is changing that, and it already works: it's
  live on mainnet today, clearing real trades by routing FX through
  aggregators — pick the currency you want on the globe and the swap
  settles. [play the mainnet demo video]"
- **Note:** Lead with the mainnet demo, per Mert's "put your best point
  first": the strongest thing we have is that it already works on the
  real network. Keep the claim exact — today we clear by routing through
  **aggregators**; the eCLOB (next page) is how we **bootstrap** the
  markets that have no liquidity yet. Don't assert "most liquid."

#### Page 4 — The eCLOB [DEMO VIDEO · localnet] · ~30s

- **On-slide:** Institutional-grade atomic settlement: order book
  transparency, propAMM efficiency.
- **Visual:** Two captures side by side, top-aligned — the "Market maker
  TUI" (markets list + a live book) and the compute-unit pane showing
  what a quote update costs. Badge **demo video · localnet** at the foot
  of the right column, in the space the shorter capture leaves.
- **Spoken:** "The routing works today, but the markets that don't exist
  yet need a venue — so we built one. The eCLOB gives you the liquidity
  guarantees of a central limit order book with quote updates as cheap
  as a propAMM: a maker repricing the whole book costs forty-seven
  compute units, reshaping the ladder about five hundred. That's what
  lets us bootstrap a brand-new market and onboard makers fast. \[play
  the localnet demo video: the book starts empty, the maker bots come
  on, real depth fills in within seconds, then a trade fills against
  it\]"
- **Note:** This page carries both halves of the eCLOB pitch — the
  design (cheap quotes, shown as real compute-unit numbers) and the
  proof (a market bootstrapped from empty). The video is **localnet**,
  and the badge says so: the flash-liquidity beat isn't a mainnet claim.
  Optional flourish if time allows: from the TUI, reshape the ladder or
  reprice the whole book in a single instruction.

#### Page 5 — Why this will fail · ~12s

- **On-slide:** Why Dropset will fail\*
- **Visual:** Arc, Tempo and Canton as a row of logo tiles, tinted red —
  each captioned with the chain the presenter names, since a mark isn't
  always the name (Arc is Circle's, so Circle's logo is what an audience
  recognizes). Eyebrow: "Permissioned distribution".
- **The asterisk:** deliberate, and the reason these two pages are a
  pair. It promises a footnote the audience doesn't get until the next
  page's title delivers it — "\*Why Dropset won't actually fail" — so the
  threat page reads as a setup rather than a concession. Don't explain it
  out loud; let them find it.
- **Spoken:** "The honest risk: everyone wants onchain settlement, and
  the ones with distribution are permissioning it. Arc and Tempo are
  building payment-and-settlement rails, and Canton is doing regulated
  onchain markets. Any of them could decide FX is theirs, and each
  arrives with the customers already on it."
- **Note:** This slide is deliberate — an investor respects that we
  named the threat first. The rebuttal is the very next page. Fuller
  framing in the appendix.

#### Page 6 — Why it will work · ~8s

- **On-slide:** But Dropset liquidity is public, and the biggest Solana
  DEXes face an innovator's dilemma (SOL, memes).
- **Eyebrow:** \*Why Dropset won't actually fail — the payoff to the
  previous page's asterisk.
- **Visual:** Jupiter, Orca, Raydium, Meteora and pump.fun as a row of
  logo tiles, matching the previous page's row — the public venues, whose
  attention is elsewhere.
- **Spoken:** "Their liquidity isn't public: it sits inside private or
  permissioned rails, where you can't make a market unless they let you.
  Dropset's is. And the venues that are public — Jupiter, Orca, Raydium,
  Meteora, pump.fun — are chasing SOL and memes, because that's where the
  volume is today. It's a classic innovator's dilemma: FX is too small to
  move them and big enough for us. Dropset is the open, neutral,
  composable venue — anyone can quote, anyone can trade, any app can
  integrate — and we're beating everyone to it."
- **Note:** Two rebuttals in one breath — (1) the closed rails from the
  previous page have no public liquidity, (2) the venues that *are* public
  aren't pointed at FX (innovator's dilemma, smaller market now). The
  composability angle (general-purpose Solana vs. a verticalized venue
  like Hyperliquid) is expanded in the appendix. Note this page shows the
  incumbents, not us — resist the urge to draw Dropset as a taller box
  beside them; an unlabelled shape next to real logos reads as a bug.

#### Page 7 — How we grow · ~8s

- **On-slide:** Vaults bootstrap a public FX liquidity flywheel.
- **Visual:** A curve of depth growing, over the flywheel's two ends,
  split by a rule: **upstream** the issuers (Loon, who issues CADC; AUDD
  Digital), **downstream** the demand (Altitude in banking, CargoBill in
  supply chain) — all people we've actually talked to about sourcing
  liquidity. Each tile is captioned with the company, then what they are
  to us: "Loon / CADC issuer", "Altitude / banking".
- **Spoken:** "We seed the markets ourselves the way Hyperliquid did —
  our vaults bootstrap each book, and anyone can top them off with
  inventory, so the flywheel is public rather than ours alone. It has two
  ends, and we've talked with both. Upstream are the issuers — Loon, who
  issues CADC, and AUDD Digital — who mint a currency and need it to
  actually trade. Downstream is the demand: Colosseum partners like
  Altitude in banking and CargoBill in supply chain, who need to buy FX
  onchain to settle. Connect the two ends and the depth compounds."
- **Note:** The vault framing is deliberate — *we* bootstrap it, but the
  flywheel is public and anyone can add inventory. The upstream /
  downstream split is the substance of the page: a venue needs both ends,
  and naming which is which shows we know where liquidity comes from and
  where it goes.

#### Page 8 — Team & close · ~6s

- **On-slide:** Built by people who've built exchanges.
- **Visual:** Alex + Judy, square and unframed. Each carries three lines:
  the role here, what they own, and the credential behind it — "Founder,
  DASMAC / Product · exchange design / prev. Cofounder, Econia Labs" and
  "Operations, DASMAC / Stablecoin rails · onramps · accounting / prev.
  Dragonfly Capital Partners". Wordmark in the persistent footer.
- **Spoken:** "I've built two onchain exchanges already, including an
  order book — I authored Econia on Aptos, which cleared around five
  hundred million in volume, and wrote the Solana Opcode Guide, the
  playbook for squeezing performance out of Solana programs. Judy owns
  operations end-to-end — banking, the stablecoin providers, onramps,
  and accounting. Dropset — Forex on Solana."

______________________________________________________________________

## 2. Presentation appendices

Not on slides. Mert: keep the nuance off the deck; put it here and
cover it if you get a call. This is the material to have ready when an
investor grills.

### Team, full

- **Alex — product / exchange design.** Exchange designer; has built
  two onchain exchanges (including an order book) before. Authored
  Econia, the onchain order book on Aptos (~\$500M cleared); co-authored
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
- **"Canton is a direct competitor."** — The Arc/Tempo camp is chasing
  settlement, and Canton is regulated onchain markets. But they're
  permissioned or walled — a different animal from an open, composable
  FX book.
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

### Lazy-VC questions to preempt

Have crisp one-liners ready for the questions a VC asks without reading
the deck:

- "What's the market?" → FX, the biggest market on earth (\$9T/day,
  24/5), with no liquid onchain home yet.
- "Who's using it?" → Live on mainnet now (clearing trades via
  aggregators); Colosseum partners (Altitude, Cargobill) and stablecoin
  issuers are the first FX demand. We've also spoken with providers like
  CADC and AUDD coming online on Solana who already have distribution
  networks.
- "Why you?" → We've built onchain exchanges before (Econia, ~\$500M);
  this is our domain.
- "Why now?" → Non-US-dollar stablecoins are only just arriving onchain
  (~14 currencies today, euro leading), and payments are following.

______________________________________________________________________

## 3. Formatting / structure rules

### How to read this

- **One page = one slide.** Eight pages, against a ten-page cap (see
  "Format rules").
- Each page gives: the **on-slide line** (the single big sentence the
  audience reads), the **visual** (the one big image), the **spoken
  copy** (what the presenter says — this is the real script), and a
  **time** budget.
- Total spoken time targets **~120 seconds**. The two demo beats eat
  ~55s of that, so every other page has to be fast.
- The demos are **recorded videos**, one per network, cued by a badge on
  their page (**demo video · mainnet**, **demo video · localnet**).
  Nothing on stage depends on a live network or a working room
  connection.
- Every page carries the same footer: the Dropset wordmark at the left,
  "Courtesy of DASMAC" in the middle (mirroring the frontend's own
  footer), and progress dots at the right. It isn't page content — don't
  budget words or space for it; the slide body already reserves room
  above it so a busy page doesn't crowd the credit.
- Presenter mode is **`⌘⇧P`** (`Ctrl⇧P` off macOS), not a bare `p`.
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

#### Name why it will fail, then answer the counters

Put up a **"why this will fail"** beat and don't flinch: say the honest
version an investor is already thinking ("Arc, Stripe, and Tempo are
good enough; open access isn't actually important"). Then show you've
thought it through — **surface the lazy-VC questions and answer them**,
and be ready to **reply to the counters** rather than hoping they don't
come up. An investor respects that the risk was named first and met
with an answer. The honest threat is Page 5; the answer is Page 6; the
fuller counters-and-replies live in the appendices.

### Format rules (distilled from the above)

Design it like a children's book, because a demo-day audience scrolls
it like Twitter — they will not read a word salad.

1. **Max 10 pages.** This deck is 8 — the cap is a ceiling, not a
   target, and two of the original ten pages (a "here's the stack" beat
   and a separate traction page) turned out to be saying what the
   mainnet demo already says.
1. **One big sentence per page.** No bullet lists anywhere — the built
   deck's three bullet-list pages are what this rule removed.
1. **One big image per page.** Name the image in the "visual" field.
1. **The sentences tell a story as you flip through.** Read the ten
   on-slide lines top to bottom and they should read as one arc.
1. **Super simple words.**
1. **Lead with the strongest selling point, not a template.** The
   strongest point here is *it already works, on mainnet, and I can show
   you* — so the mainnet demo is Page 3 and the arc is built around it.
   No generic problem → solution → market-size structure.
1. **No market-opportunity slide.** We do not put up a "\$9T TAM"
   slide. FX size appears once, as the *shape of the gap* ("the
   biggest market on earth barely exists onchain"), never as a
   trillion-dollar brag.

### Reference — the accelerator's 7-point pitch structure

The Colosseum "basic pitch" framework, from the pitch review
in the fundraise tracker. Not the deck's structure — Mert's
children's-book arc wins for a 2-minute demo — but every point below
must be *covered* somewhere, and this is the checklist the accelerator
expects. Mapping to our pages in brackets.

1. **One-liner.** DASMAC is building Dropset, an onchain Forex platform
   that harnesses Solana for open, efficient exchange of multinational
   currencies at scale. [Pages 1, 8]
1. **Problem / unique insight.** ~14 currencies now live on Solana via
   stablecoins; Solana settlement can support the massive FX market
   *composably* — DevEx convenience for payments providers, merchants,
   manufacturers, and retail — because Solana is general-purpose, not
   verticalized like Hyperliquid. [Pages 2, 6; appendix]
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
1. **Founders' bio.** Exchange-design background — authored the Econia
   order book (~\$500M on Aptos) and the Solana Opcode Guide — with a
   dedicated operations owner (Judy) on banking and accounting. Full
   detail on Page 8 and in the appendix (kept there to stay DRY).
   [Page 8; appendix]
