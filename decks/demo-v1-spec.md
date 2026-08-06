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

This is **outline v2**, revised twice against review. What changed from
the deck that first went out:

- Full sentences on slides, in place of fragment headlines.
- **Static images only.** The two recorded demos and the click-to-play
  badge are gone; product beats are interface screenshots.
- **Ten pages, not eight.** Pages 3–6 are now one argument in
  sequence: the swap works today → we curate the data for every currency
  → most of them have no liquidity at all → the eCLOB is what we're
  building to fix that. Splitting those beats is what makes the eCLOB
  land as an answer to a problem the audience has just been shown.
- A **growth roadmap** page (three beats in time order), answering the
  revenue question the first deck left implied.
- The "why this will fail" / "why it won't" pair **collapsed into one
  page**, and the asterisk device is retired.

______________________________________________________________________

## 1. Slide contents

### The 2-minute narrative (continuous read)

The through-line, so the story reads as one piece before it's cut into
pages:

> Dropset is where currency trades onchain, built by DASMAC. Foreign
> exchange is the biggest market on earth — over nine trillion dollars a
> day — but it trades only 24/5, banks and over-the-counter desks fragment
> its liquidity, and less than ten percent of the
> world's currencies are even available on Solana today. This already
> works: Dropset already processes Solana mainnet FX trades — you open the
> picker, select your currency, and the swap settles atomically, at
> dropset.io/swap right now. Alongside that, Dropset curates the market
> data for every Solana-based currency — and when you sort
> by liquidity, the bottom of that list is currencies with no market
> whatsoever: the Australian dollar, the Canadian dollar, the yen, the
> naira, the lira. So we're building the exchange those markets need.
> Dropset ships propAMM efficiency and CLOB transparency, where
> repricing the whole book costs forty-seven compute units and
> reshaping the ladder fifty-nine. FX vaults bootstrap a public liquidity
> flywheel, and it's a two-sided market we're already doing the customer
> development on — upstream the issuers who need their currency to trade,
> downstream the payments companies who need to buy FX to settle. The path
> from here is deliberate: DASMAC bootstraps nascent FX pairs by leading
> Hyperliquid-style vaults now, protocol fees accrue value as markets
> mature, and derivatives provide an expansion opportunity once spot is
> fully mature. Permissioned onchain liquidity has an adoption ceiling:
> permissioned solutions block composability, and no fintech is going to
> settle on a competitor's private ledger — Dropset is open and composable
> on Solana, the most money-like onchain environment, where ease of
> transmission and composability are maximized. Public liquidity is
> what blockchains were built for.
> Dropset is built by people who have built exchanges. Dropset — where
> currency trades onchain.

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
  three chevron-marked facts: daily volumes exceed \$9 trillion; banks and
  OTC desks fragment liquidity; less than 10% of the world's currencies are
  available on Solana. Each is a **clause with a subject and a verb** —
  they were fragments ("over \$9 trillion daily volume"), which read as a
  spec sheet rather than as three things being said.
- **Visual:** A **progress bar** — 8.6% of currencies available on Solana
  — over the currencies count from our own site, captioned
  `dropset.io/currencies`.
- **Spoken:** "Foreign exchange is the biggest market on earth — over
  nine trillion dollars a day. But it only trades 24/5, banks and
  over-the-counter desks fragment its liquidity, and less than ten percent
  of the world's currencies are even available on Solana today: fourteen
  out of a hundred and sixty-two, and that count is live on our own site,
  which is where this is from. Every currency should be connectable to
  every other one, and that's what we're building. To be precise: we don't
  issue currencies — issuers create them, and Dropset is where they trade."
- **Note:** Frame it as **gap plus upside**, never a market-size slide.
  The every-currency vision beat starts here, worded as **connection,
  not issuance** — Dropset does not issue currencies; issuers create them
  and Dropset is where they trade. The ~\$9T/day figure needs no citation
  (it isn't disputed at pitch-deck level), but the currency count keeps
  its attribution because it's ours and it's checkable. **Do not invent a
  Solana volume-share percentage.**
- **Note on the three facts:** they're the deck's one deliberate
  exception to "no bullet lists" — three facts that are peers, which the
  audience should take at a glance and which prose would bury. They are
  **not** an exception to full sentences: each carries a subject and a
  verb, so the list reads as three statements rather than as a spec sheet.
  The marker is a **chevron**, not a disc (a row of
  discs is what makes a slide read as a corporate template) and
  deliberately not a literal `≥`: that glyph makes a numeric claim, and
  next to the third fact — a *less-than* — it would read as a
  contradiction.
- **Note on the bar:** a single ratio against a limit is a **meter**, not
  a pie of two slices — the empty part of the track *is* the message. It
  carries the **percentage only**; the raw 14-of-162 count belongs to the
  screenshot beneath it, which is the **citation**: our own page, showing
  the number, with the URL. Labelling both restated the same figure
  twice.

#### Page 3 — Live today · ~12s

- **On-slide:** Dropset already processes Solana mainnet FX trades.
- **Visual:** The swap flow **left to right, one step per column**, the
  steps being "open the picker", "select your currency" and "swap
  atomically" — kept terse so each holds one line at a size readable from
  the back of a room. Each caption sits **above** its own capture, and
  the columns (and the chevrons between them) are **vertically centred**.
  The three captures get progressively taller, so centring makes the
  labels climb like steps — which is the effect worth keeping. Captions
  underneath, by contrast, landed on three different baselines and read as
  ragged. The URL — `dropset.io/swap` — sits **under the middle step**, in
  the space that shorter column leaves.
- **Spoken:** "This already works. Dropset already processes Solana
  mainnet FX trades: you open the picker, select your currency, and the
  swap settles atomically. The ramps are near instant and the venue never
  closes. Solana is the start, not the end — it's the most
  moneyness-conducive environment onchain. And it's on dropset.io/swap
  right now, so you can go and do this yourself."
- **Note:** The *why onchain matters* beat lives here, spoken. Keep the
  claim exact: today we clear by routing through aggregators and sourcing
  existing liquidity. Don't assert "most liquid". The **URL is doing real
  work** — the captures prove the flow exists, and the link says the
  audience can go do it themselves, which is the same job
  `dropset.io/currencies` does on the gap page. The globe is **not**
  the way in — an earlier draft framed the flow as picking a country off
  the globe, which isn't how anyone uses it; the globe appears in the
  third capture as the route being drawn, which is what it's for.
- **Note on height:** this is the **tightest page in the deck**, and the
  numbers matter. The third capture is 820×1371, so it sets the row's
  height and the row is most of the page. At a 410-unit step width, with a
  two-line statement and the URL below the row, the page stacked to ~1008
  units against the ~910 a slide has — and because slide content is
  flex-centred, the overflow split top and bottom and **cropped the
  eyebrow off the top edge**. Three things brought it back: a shorter
  statement that fits one line, a narrower step width, and moving the URL
  into the middle column so it stops adding to the page's height. The first
  two later relaxed once the heading was pinned with `nowrap` rather than
  estimated, but if this page grows again, that is the order to give ground
  in. The live figure is `DemoDeck.tsx`'s `STEP_WIDTH`; don't restate it
  here, where it goes stale.

#### Page 4 — Currency curation · ~10s

- **On-slide:** Dropset curates market data for all Solana-based
  currencies.
- **Visual:** **One** capture, as large as the page allows — every
  currency sorted by onchain liquidity, deepest first, with price, 24h
  change and volume, market cap, liquidity and holders.
- **Spoken:** "And alongside the swap itself, Dropset curates the market
  data for every Solana-based currency: price, twenty-four-hour change and
  volume, market cap, liquidity, holders — grouped by country, or sorted
  however you want. This is sorted by liquidity, deepest first."
- **Note:** A continuation of page 3, not a new topic — same product,
  second capability. **One table, blown up.** An earlier version put
  three tables on this page (grouped by country, sorted by liquidity, and
  the tail) and none of them could be read at that size; a group-by-country
  capture was dropped for the same reason.

#### Page 5 — The long tail · ~10s

- **On-slide:** Many currencies have no liquidity whatsoever.
- **Visual:** The tail of that same table, full width — the Australian
  and Canadian dollars, the yen, the naira, the lira and more, every
  column showing a dash.
- **Spoken:** "Scroll to the bottom of that same list and the story tells
  itself. The Australian dollar, the Canadian dollar, the yen, the naira,
  the lira — all sitting there with no price, no volume, and no liquidity
  at all. These are real currencies with real economies behind them, and
  onchain they have no market whatsoever."
- **Note:** This page is the **hinge**, and it's why the curation beat
  got split off page 4: the eCLOB has to arrive as the answer to a
  problem the audience has just been shown in our own data, rather than
  as a design we assert is needed. Don't fold it back into page 4 to save
  a page — the pause on this slide is the setup.

#### Page 6 — The eCLOB · ~18s

- **On-slide:** Eyebrow "The eCLOB", then one sentence: Dropset ships
  propAMM efficiency and CLOB transparency. (CLOB, the acronym — not
  "order-book", which spells out a term the page has already named.)
- **Visual:** Three captures **side by side, vertically centred**, each
  captioned underneath: "Reprice: 47 CU · reshape: 59 CU" (the
  compute-unit pane), "Demo maker quoting locally" (the maker's control
  panel), and "Liquidity routes to the frontend" (the order book, live
  trades tape and a priced swap on the product itself). No connecting
  chevrons — left to right already reads as cost → maker → product without
  being told to.
- **Spoken:** "So we're building the exchange those markets need. Making
  a market onchain used to be prohibitively expensive — gas made
  continuous quoting impossible, so everything before this was a
  band-aid. We've built order books before, so we built one that fits:
  the eCLOB gives you the transparency of a central limit order
  book with quote updates as cheap as a propAMM. Repricing the whole
  book costs
  forty-seven compute units and reshaping the ladder fifty-nine, on a
  chain that gives you two hundred thousand per instruction. Left to right:
  that's what a quote costs, that's our own maker paying it to quote a live
  market, and that's the same liquidity arriving on the frontend with the
  book, the trades tape and a priced swap. We're building this out so
  anyone can quote onchain with a vault-style approach."
- **Note:** **One short sentence, and it must not wrap.** This heading is
  the page's whole height budget: it went through four drafts, each of
  which wrapped one line further than intended, and each time the overflow
  clipped this slide's own eyebrow off the top. The current copy is short
  enough *and* pinned with `nowrap`, so the browser enforces what the
  budget assumes rather than the author estimating text metrics. The
  compute-unit numbers live on the capture that shows them rather than
  being restated in the heading.
- **Note on the row:** left to right is **low-level → system → product**:
  the cost of a quote, the maker paying that cost, the liquidity showing up
  on the frontend. It needs no chevrons to say so — an intermediate draft
  added them and they only made the row busier. Three columns of one
  capture each is also far shorter than two stacked in a column, which is
  the change that finally gave this page real headroom. It replaced a strip
  of four small keyframe thumbnails — one screenshot of the thing working
  says more than four stills of it starting up, and needs no localnet
  capture session to produce.

#### Page 7 — How we grow · ~15s

- **On-slide:** FX vaults bootstrap a public liquidity flywheel.
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

#### Page 8 — Growth roadmap · ~15s

- **On-slide:** Our path to expansion is deliberate and methodical. Then
  three
  beats in time order, spanning the page:
  1. **Now** — DASMAC bootstraps liquidity. DASMAC bootstraps nascent FX
     pairs by leading Hyperliquid-style vaults using the Dropset protocol.
  1. **Next** — protocol fees accrue value. As markets mature, volume and
     fees compound, and currency pairs achieve deep liquidity.
  1. **Later** — derivatives provide an expansion opportunity. Once spot
     is fully mature, hedging instruments and additional derivatives
     enable more efficient market making and more mature markets.
- **Visual:** The three beats as a rollout along one unbroken rule spanning
  the full page width, not a static list. Each dot sits **directly above
  its own beat's heading**.
- **Note on the dots:** their spacing is **derived from the column pitch**,
  and it has to stay that way. They were originally a flex row of three
  equal segments while the text below was `space-between` on a fixed column
  width — two different geometries, agreeing only on the first column, so
  the second dot sat 27 units left of its heading and the third 53. Both
  rows now come from one pitch, so changing the column width moves the dots
  with it.
- **Note:** Called a **roadmap** rather than "commercial viability" — the
  growth story is the frame, and "viability" invites the question of
  whether it is viable. The headline is a *path*, not a funding
  mechanic: an earlier draft read "each stage funds the next", which made
  the page about cashflow rather than about direction. The rollout shape
  matters — a static list reads as speculation, three beats in time order
  read as a plan. This is also where the **DASMAC / Dropset** distinction
  is made explicit. Name the streams in abstracted language, not jargon —
  no "fee switch".

#### Page 9 — Why the open venue wins · ~12s

- **On-slide:** Permissioned onchain liquidity has an adoption ceiling.
  Two panels: the permissioned rails (Arc, Canton, Tempo, red-outlined),
  captioned "Permissioned solutions are blocking composability.
  Competitive dynamics prevent fintech companies from adopting a
  competitor's private ledger."; and the Dropset wordmark, green-outlined,
  captioned "Dropset is open and composable on Solana, the most money-like
  onchain environment, where ease of transmission and composability
  are maximized. Public liquidity is what blockchains were built for."
  Both badges are **top-aligned** so they sit on one line — see the note
  below.
- **Spoken:** "Permissioned onchain liquidity has an adoption ceiling.
  Arc and Tempo are building payment-and-settlement rails, and Canton is
  doing regulated onchain markets — any of them could decide FX is
  theirs, and each arrives with the customers already on it. But their
  liquidity isn't public: you can't make a market unless they let you,
  and that blocks composability for everyone downstream. And competitive
  dynamics stop it before it starts: a fintech isn't going to settle on a
  competitor's private ledger. A bank that competes with Circle won't
  build on Arc, and a multi-signature banking product isn't going to run
  on Canton. Dropset is open, neutral and composable: anyone can quote,
  anyone can trade, any app can integrate. Public liquidity is what
  blockchains were built for — moving money is the problem they were
  supposed to solve, and this is that. And that's why we started on Solana:
  the most money-like onchain environment there is, where ease of
  transmission and composability are both maximized."
- **Note:** The v1 fail / won't-fail **pair collapsed into this one
  page**, and the asterisk device is retired — it cost a page and the
  payoff didn't carry. The framing is now an **adoption ceiling** rather
  than "what FX's consumers need", which was a claim about other people's
  requirements. A ceiling is the sharper version of the same point: it
  isn't that permissioning is unpleasant, it's that it caps how far the
  thing can ever get. The
  second clause on the permissioned caption is the sharper half of the
  argument, and it's a **commercial** point rather than a technical one: a
  fintech will not settle on a competitor's private ledger. A bank that
  competes with Circle won't build on Arc; a multi-signature banking
  product won't run on Canton. The permissioning doesn't just restrict
  access, it disqualifies the people who most need the rail — and those
  examples belong in the spoken track, not on the slide.
- **Note on alignment:** the two badges are **top-aligned**, and that has
  to stay explicit. Spectacle's `FlexBox` defaults to `alignItems: center`,
  which vertically centred each panel *including its caption* — so the
  panel with the longer caption became the taller column and its badge rode
  up relative to the other. Aligning to the top puts both badges on one
  line, which is what makes the pair read as a comparison rather than as
  two unrelated boxes.
- **Note on the logos:** this page is the **one exception** to keeping
  competitor marks off the deck, and it is deliberate. The red outline is
  what makes it work — the marks are labeled as the unfavorable case
  rather than presented neutrally, so the row argues instead of just
  listing, and the Dropset wordmark opposite it is the answer. An
  intermediate draft made this page abstract (a gated panel versus a hub
  diagram, nobody named); it was less legible than the real marks, which
  an audience recognizes instantly. The **Solana DEXes stay off-slide** —
  that argument is an innovator's-dilemma point that needs a sentence,
  not a logo row.

#### Page 10 — Team & close · ~8s

- **On-slide:** Eyebrow "The team", then "Dropset is built by people who
  have built exchanges" — matching every other page's kicker-plus-sentence
  shape. Then one line each: Alex Kahn, Founder — authored two exchanges
  on Aptos, including the Econia order book (\$500M lifetime volume);
  authored the Solana Opcode Guide, the definitive resource for optimizing
  Solana program efficiency. Judy Sosa, Operations — owns the whole
  operational stack, working with banks, stablecoin providers, onramps and
  service providers.
- **Visual:** Both headshots, square and unframed, pulled from the
  marketing site at build time (`remote-assets.json`).
- **Spoken:** "Dropset is built by people who have built exchanges. I
  authored two on Aptos, including the Econia order book, five hundred
  million dollars of lifetime volume, and I authored the Solana Opcode
  Guide, the definitive resource for optimizing Solana program efficiency
  — which is what makes quoting on the eCLOB cost double-digit compute
  units. Judy owns the whole operational stack, and works directly with
  banks, stablecoin providers, onramps and service providers. Dropset —
  where currency trades onchain."
- **Note:** **State what each person has done; don't argue for why the
  role matters.** An intermediate draft justified the operations split
  ("this is the work that gets an FX venue integrated with the rails…",
  "a dedicated owner rather than a founder's side task") — that reads as
  defending the team, and it framed one person's work relative to the
  other's rather than on its own terms. One sentence each, both in the
  same voice. The credential reads "Dragonfly Capital", not "…Partners",
  with the EA role stated plainly. The final spoken line mirrors the
  title. Because this page lingers on screen after the talk, it's the one
  place slightly longer copy is correct — but only slightly.

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

### The competitors — the fuller answers

Page 9 names and shows the permissioned rails. These are the answers
behind that page, plus the one competitor set that stays off-slide
entirely:

- **The settlement chains (Arc, Tempo) and regulated onchain markets
  (Canton).** On-slide, red-outlined. Each is chasing onchain settlement
  and each arrives with customers already on it. The answer: they're
  private or heavily permissioned rails. The moment FX needs a *neutral*
  venue where anyone can make a market and anyone can trade, a closed
  garden can't serve it — and in the meantime the permissioning is what
  blocks composability for everyone building downstream.
- **The existing Solana DEXes (Jupiter, Meteora, Orca, pump.fun,
  Raydium) — off-slide.** They aren't focused on FX, and we're beating
  them to it. It's an innovator's dilemma: the volume today is too small
  to move a giant and big enough for a focused team, and we'll be here
  for the next 10x as payments come onchain. Their customer is a
  different customer — the retail speculator, not the business that needs
  to settle an invoice in another currency. This one stays off the deck
  because it needs that sentence to land; a logo row would just look like
  a list of people beating us.
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
- "How do you make money?" → Page 8 is the answer, and the appendix
  detail is that each stage compounds into the next: liquidity
  operations now, protocol fees as the books thicken and volumes
  compound, derivatives once there's enough depth to hedge against.

______________________________________________________________________

## 3. Formatting / structure rules

### How to read this

- **One page = one slide.** Ten pages, at a ten-page cap (see "Format
  rules").
- Each page gives: the **on-slide line** (what the audience reads), the
  **visual** (the one big image), the **spoken copy** (what the
  presenter says — this is the real script), and a **time** budget.
- Total spoken time targets **~120 seconds**. With the demo videos gone,
  the budget is spread across the pages rather than concentrated in two
  of them.
- Every page carries the same footer: the Dropset wordmark at the left,
  the "Built by DASMAC" credit in the middle, and progress dots at the
  right. It isn't page content — don't budget words or space for it; the
  slide body already reserves room above it.
- Presenter mode is **`⌘⇧P`** (`Ctrl⇧P` off macOS), not a bare `p`.
- Anything nuanced — the fuller competitor answers, the investor
  grilling, the numbers behind a claim — is **not on a slide**. It lives
  in the appendices (section 2) and only comes out if a conversation goes
  there.

### Global rules — v2

These are firm, and they override the older guidance where the two
disagree:

1. **Full sentences on-slide**, everywhere, including inside the one list
   the deck allows (page 2's three facts). No fragments — not as headlines,
   not as list items. A reviewer reading the deck without the talk should
   get the argument.
1. **No terminal period on a headline — on any page, at any level.** At
   display size a full stop is a visible mark that earns nothing: there is
   no following sentence for it to separate. Sentence *structure* still
   applies (rule 1); only the period goes. This covers all ten page
   headlines, the title's "Built by DASMAC" credit, **and the roadmap's
   three beat headlines** — anything that reads as a title rather than as
   prose. Multi-sentence copy — the venue captions, the roadmap bodies, the
   team bios — keeps its punctuation, because there the period is doing its
   actual job.
1. **16:9 aspect ratio**, set explicitly on the deck rather than
   inherited.
1. **Static images only.** No embedded video, no gifs, no player. A
   product beat is an interface screenshot with a claim over it. This
   retires the click-to-play badge and the two recorded demos.
1. **Logos are argued, never listed.** Partner marks (page 7) are
   captioned with what the company is to us. Competitor marks appear on
   exactly one page (page 9), red-outlined as the unfavorable case, with
   the Dropset wordmark opposite as the answer — a neutral row of
   competitor logos hands them the frame, but a row that is visibly the
   thing being argued against does the opposite. The Solana DEXes stay
   off-slide entirely, because that argument needs a sentence.
1. **Solana is never framed as a ceiling.** It's the deliberate start —
   the most money-like environment onchain, with the highest ease of
   transmission — never the boundary.
1. **DASMAC is the company, Dropset is the protocol.** The distinction
   has to be legible on the slides: "Built by DASMAC" on the title
   carries it, and the roadmap attributes the bootstrap beat to DASMAC
   explicitly.
1. **No bullet lists**, bar page 2's three facts. Everywhere
   else, prose.

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

**If a page's eyebrow looks cut off at the top, the page is overflowing.**
Slide content is flex-centred in the ~910 units a slide has, so an
overflow splits between top and bottom and takes the kicker with it —
which reads as a missing title rather than as too much content. In
descending order of cheapness, the levers are: pin the heading to one line
(`nowrap` on `Statement`, worth ~70 units and the usual culprit), shorten
the heading, lay stacked captures out side by side instead, then shrink the
captures. Estimating text metrics by eye is what caused every instance of
this; prefer the constraint the browser enforces.

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
holds, but full sentences replaced fragments everywhere, and page 10
carries a line per person because it lingers on screen after the talk. A
reviewer reading only the deck is now a first-class case.

#### Name why it will fail, then answer the counters

Put up the honest risk and don't flinch, then show it's been thought
through — surface the lazy-VC questions and answer them, and be ready to
reply to the counters rather than hoping they don't come up. An investor
respects that the risk was named and met with an answer.

In v1 this was a two-page setup-and-payoff pair with an asterisk gag.
In v2 it is **one page** (page 9): the pain point and the answer in the
same breath. The steelman versions and the replies live in the appendix.

### Format rules (distilled from the above)

1. **Max 10 pages.** This deck is 10 — at the cap. Anything added from
   here has to displace something.
1. **One big sentence per page.** No bullet lists, bar the one page-2
   exception.
1. **One big image per page.** Name the image in the "visual" field. A
   page may show several captures of *the same thing* (the swap flow, the
   maker and the frontend it feeds) — that is still one image in the
   sense that matters, because it's one idea. It does **not** permit three
   unrelated tables on a page; that was tried and none of them could be
   read.
1. **The sentences tell a story as you flip through.** Read the ten
   on-slide lines top to bottom and they should read as one arc. Pages
   3–6 are the load-bearing stretch: works today → we curate the data →
   the tail is empty → here's the exchange that fixes it.
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
   currencies at scale. [Pages 1, 10]
1. **Problem / unique insight.** ~14 currencies now live on Solana via
   stablecoins; Solana settlement can support the massive FX market
   *composably* — DevEx convenience for payments providers, merchants,
   manufacturers, and retail — because Solana is general-purpose, not
   verticalized like Hyperliquid. [Pages 2, 5, 9; appendix]
1. **Solution / product.** Dropset routes existing onchain liquidity
   through aggregators and adds a novel eCLOB to bootstrap new markets
   with inexpensive quote updates that accelerate market-maker
   onboarding. [Pages 3, 6]
1. **Traction.** Dropset.io is live and clearing trades on mainnet
   (today via aggregators), and curates the market data for every
   currency on Solana. [Pages 3, 4]
1. **Why the market is massive.** FX is >\$9T/day and 24/5; Solana as
   intermediary gives atomic settlement and faster on/off-ramps. \[Page
   2\]
1. **Why now.** The non-US stablecoin market has only just started to
   expand — EUR stablecoins drive most volume, more currencies going
   live (14 on Solana). [Pages 2, 4]
1. **Business model.** Liquidity operations now, protocol fees as
   volumes compound next, derivatives after that. [Page 8]
1. **Founders' bio.** Exchange-design background — authored the Econia
   order book (~\$500M on Aptos) and the Solana Opcode Guide — with a
   dedicated operations owner on banking and accounting. Full detail on
   Page 10 and in the appendix (kept there to stay DRY). \[Page 10;
   appendix\]
