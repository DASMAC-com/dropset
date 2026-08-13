<!-- cspell:word Babelfish -->

<!-- cspell:word cofounded -->

<!-- cspell:word composably -->

<!-- cspell:word cryptobriefing -->

<!-- cspell:word Dragonfly -->

<!-- cspell:word emojicoin -->

<!-- cspell:word fundraise -->

<!-- cspell:word Genfinity -->

<!-- cspell:word Mert -->

<!-- cspell:word solanafloor -->

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

This is **outline v3**, revised against the accelerator's sign-off review.
What that round changed:

- **The deck gets sparser, not longer.** The reviewer's summary was that a
  concise deck still had two pages doing too much work. The gap page split
  into two, the roadmap's paragraphs moved to the talk track, and the
  open-venue page was rebuilt around one argument instead of a comparison.
- **Competitors leave the deck entirely.** No permissioned-rail logo, no
  competitor name, on any slide **or** in any spoken track. The argument
  they used to anchor is now made positively — new asset classes consolidate
  on Solana, and FX is next — so nobody gets free press and the wind-up
  disappears. This retires v2's one sanctioned competitor-logo row.
- **The pitch opens with momentum.** Two short pages — huge market, no
  penetration — in place of one page carrying six facts.
- **The founder is part of the pitch.** A closing page replays the title and
  stays up, and its talk track carries the why-me beat: people invest in the
  founder at least as much as in the problem, and v2 gave them nothing on it.

What v2 established, and this round keeps:

- **DASMAC recedes.** The company is the boring DevCo in the background,
  discoverable when someone signs a document. It is named exactly once, in
  the footer credit — off the title slide, off the roadmap, out of the team
  roles. Dropset is the brand.
- **Money-ness is the through-line.** The gap pages (2–3) state the problem
  in money-ness terms and name the ambition — 24/7/365 coverage of every
  spot pair — and the flip page (10) answers in the same terms.

What changed from the deck that first went out:

- Full sentences on slides, in place of fragment headlines.
- **Static images only.** The two recorded demos and the click-to-play
  badge are gone; product beats are interface screenshots.
- **Twelve pages, not eight.** Pages 4–7 are one argument in
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

> Dropset is the liquidity layer for every national currency — all of them,
> for anyone, 24/7/365. Anyone with a phone can buy tokenized Tesla stock
> today, from almost any country — and nobody can do that with a euro.
> Foreign exchange is the biggest market on earth:
> over nine trillion dollars a day. But it barely trades onchain — less than
> ten percent of the world's currencies are on Solana, the fastest
> and cheapest chain, and our goal is 24/7/365 coverage of every FX spot
> pair. This already works: Dropset already processes Solana mainnet FX
> trades — you open the picker, select your currency, and the swap settles
> atomically, at dropset.io/swap right now. Alongside that, Dropset curates
> the market data for every Solana-based currency — and when you sort by
> liquidity, the bottom of that list is currencies with no market
> whatsoever: the Australian dollar, the Canadian dollar, the yen, the
> naira, the lira. So we're building the exchange those markets need.
> Dropset ships propAMM efficiency and CLOB transparency, where repricing
> the whole book costs forty-seven compute units and reshaping the ladder
> fifty-nine. FX vaults bootstrap a public liquidity flywheel, and it's a
> two-sided market we're already doing the customer development on —
> upstream the issuers who need their currency to trade, downstream the
> payments companies who need to buy FX to settle. This is the path to
> 24/7/365 FX and beyond: we onboard emerging stablecoin issuers, protocol
> fees accrue value, and the product expands beyond spot. And FX is next,
> because every new asset class has consolidated here already. First
> memecoins — over twelve million tokens launched, the training wheels, and
> the proving ground that anyone could launch a market here and anyone could
> trade against it. Then tokenized equities, and that one is a big deal:
> ninety-six percent of that volume onchain is on Solana, and effectively
> nowhere else. FX is next,
> because it needs exactly what this chain has — public liquidity, where
> anyone can quote and anyone can trade against it, so depth compounds
> instead of sitting still. Permissioned rails gate that access, and gated
> liquidity never compounds. Public liquidity is what blockchains were built
> for. Dropset is built by people who have built exchanges. Dropset — the
> liquidity layer for every national currency.

### Page-by-page

#### Page 1 — Title · ~5s

- **On-slide:** The liquidity layer for every national currency. Nothing else.
- **Visual:** The Dropset wordmark, alone.
- **Spoken:** "Dropset is the liquidity layer for every national currency —
  all of them, for anyone, 24/7/365. Here's what I mean. Anyone with a phone
  can buy tokenized Tesla stock today, from almost any country — and nobody
  can do that with a euro. That's the hole we're filling."
- **Note on the tagline:** two predecessors lost. "Where currency trades
  onchain" described the product accurately and asked the audience to care on
  its own. "The 24/7/365 currency translation layer" then carried the
  ambition, but it **wrapped on the slide** — it read as "The Currency
  Translation Layer 24…" — and translation is the metaphor for the mechanism
  rather than the thing being sold. **Liquidity** is the business: every
  currency liquid, for everyone, always, which is what Dropset offers
  stablecoins and the platforms holding them. "national" is load-bearing —
  to a crypto-native audience "every currency" reads as every *token* ("you're
  making my token liquid?"), and the one word that disambiguates is worth the
  extra width. The always-on claim moved into **this page's spoken opener**,
  which is what keeps page 2's 24/5 fact a payoff rather than an orphan; page
  12's opener stays bare, since it hands straight off to the personal why and
  the close should land on the problem rather than re-pitch a feature.
  Candidates that lost: "Fiat's final frontier" (evocative, says
  nothing about what we do) and "Babelfish for fiat currency" (the analogy is
  right and the reference dates the speaker).
- **Note on the analogy:** the tokenized-equity comparison is **spoken, never
  printed**. It is the sit-up moment — a VC who knows nothing about FX
  instantly understands that one asset class crossed onchain and the other
  didn't — and it sets up page 10, which returns to tokenized equities with
  the number attached. On the slide it would be a second sentence competing
  with the tagline; said out loud over a bare wordmark, it lands.
- **Note:** **Dropset is the brand; DASMAC recedes.** The company is the
  boring DevCo in the background — discoverable when someone signs a
  document, not something a title slide argues for. So this page carries
  no "Built by DASMAC" line and no company banner: the footer credit is
  the deck's one mention of DASMAC, and it is enough. This supersedes v2's
  earlier title-slide credit and the review note that asked for company
  brand art here, both of which are won't-do. Solana is **not** mentioned
  here either; the old "Forex on Solana" line implied a boundary the deck
  no longer wants. The deck now **closes on a replay of this page** (page
  12), which supersedes v2's "no separate closing slide" — see that page
  for why.

#### Page 2 — A huge market · ~7s

- **On-slide:** Eyebrow "The market", then "Foreign exchange is the biggest
  market on earth". Then one
  figure, very large: **\$9T+**, labeled "traded every day". Nothing else.
- **Visual:** The figure *is* the visual. No meter, no capture, no facts.
- **Spoken:** "Foreign exchange is the biggest market on earth — over nine
  trillion dollars a day. It's also structurally old: banks and
  over-the-counter desks fragment the liquidity, and it only trades 24/5 —
  it closes on Friday afternoon and it doesn't open again until Sunday
  night."
- **Note:** This page and the next are **one open, split in two**, and the
  split is the point. The v2 gap page carried a sentence plus six
  chevron-marked facts plus a meter plus a screenshot — accurate, and the
  densest thing in a deck that is otherwise concise. A pitch's first content
  page sets the pace for everything after it, so this one now carries a
  single number and gets out of the way: **huge market** here,
  **no penetration** on page 3.
- **Note on what left the slide:** fragmentation and 24/5 are now **spoken
  only**. Both are real and neither is the beat — the beat is the size of
  the prize — and as printed facts they invited the audience to read a list
  while the presenter talked. They come back out loud, where the "closes on
  Friday" line does more work than a bullet reading "FX markets only trade
  24/5" ever did.
- **Note on the figure:** the ~\$9T/day number needs no citation — it isn't
  disputed at pitch-deck level — which is exactly why it can be the whole
  page. **Do not** add a market-size breakdown, a CAGR, or a TAM ring here;
  this is the shape of the gap, not a market-opportunity slide (see the
  format rules). And **do not invent a Solana volume-share percentage**.

#### Page 3 — No penetration · ~10s

- **On-slide:** Eyebrow "The gap", then "But it barely trades onchain".
  Then the meter and its
  citation, and nothing else.
  Note these two pages are the only ones whose eyebrow differs from this
  doc's page title — every other page uses its title verbatim as the
  eyebrow. "A huge market" / "No penetration" name what the pages *do* in
  the argument, which is what a spec heading is for; "The market" / "The
  gap" are what the audience should read, which is what an eyebrow is for.
- **Note on the headline — it is half a sentence, deliberately.** Read in
  sequence, pages 2 and 3 are **one sentence split across two slides**:
  "Foreign exchange is the biggest market on earth" / "but it barely trades
  onchain". That is what makes the split read as one open with momentum
  rather than as two market-size slides, and it is why this line opens on a
  conjunction. Reword the pair together or not at all.
  Two rejected drafts and why: "Blockchains have almost none of it" asks the
  audience to carry "it" across a slide boundary and makes *blockchains* the
  subject, where the page is about FX; "But almost none of it settles
  onchain" narrows the claim to settlement, when the gap is that FX barely
  trades there at all.
- **Visual:** A **progress bar** — 8.6% of currencies available on Solana —
  over the currencies count from our own site, captioned
  `dropset.io/currencies`.
- **Spoken:** "And blockchains have almost none of it. Less than ten percent
  of the world's currencies are on Solana — fourteen out of a hundred and
  sixty-two — and that count is live on our own site, which is where this is
  from. Public blockchains are the most money-like digital environment we
  have, and Solana most of all: it's the fastest and the cheapest. So the
  gap is the whole opportunity. Our goal is 24/7/365 coverage of every FX
  spot pair — every currency connectable to every other one. To be precise:
  we don't issue currencies — issuers create them, and Dropset is where they
  trade."
- **Note:** The **statement carries the claim** that used to be the fifth
  chevron fact, so the page needs no fact list at all: the meter shows the
  ratio, the screenshot cites it, and the sentence says what it means.
  That is what makes this page as sparse as page 2 while still carrying the
  deck's most checkable number.
- **Note on the goal line:** the 24/7/365 ambition is **spoken here, not
  printed**, and that is a change from v2 where it was the page's one accent
  row. The title page's spoken open still carries 24/7/365, so printing it
  again three pages later reads as a deck repeating itself rather than as an
  escalation. This note used to say "restore the accent row only if the
  tagline ever loses the number" — the tagline has since lost it, and the row
  stays retired anyway: the always-on claim moved into page 1's spoken opener
  rather than off the deck, so the payoff this page needs is still set up, and
  page 3's sparseness is worth more than a second printing of a number the
  room has already heard.
- **Note on the money-ness fact:** "public blockchains are the most
  money-like digital environment available today" was v2's **thesis fact**
  and it is now spoken. It is still the claim the rest of the deck answers
  to — page 10 is its payoff — but as a printed row it was the abstract
  sentence on a page whose job is a concrete ratio. Keep it in the talk
  track and keep it about the *class* of environment, with "especially
  Solana" as the qualifier.
- **Note on the bar:** a single ratio against a limit is a **meter**, not
  a pie of two slices — the empty part of the track *is* the message. It
  carries the **percentage only**; the raw 14-of-162 count belongs to the
  screenshot beneath it, which is the **citation**: our own page, showing
  the number, with the URL. Labelling both restated the same figure
  twice.
- **Note on the count:** `LISTED_CURRENCIES` / `TOTAL_CURRENCIES` in
  `DemoDeck.tsx` drive both the meter and the spoken figure. They are live
  numbers from our own site, so **check them before presenting** — the
  spoken track says "fourteen out of a hundred and sixty-two" and a stale
  slide next to a corrected number read aloud is worse than either alone.
- **Note on the block's size — the capture is the cap.** Splitting the gap
  page gave this one the whole width, so the meter grew from 760 to 900. It
  cannot grow further: `currencies-listed.png` is **876 px wide**, the
  smallest capture in the deck, and at the full ~1180 measure the frame asks
  for 1142 units of a 876-px image and visibly blurs in the export while
  every neighboring capture stays crisp. At 900 the image renders at 862 and
  is still being downscaled. **Re-shoot the capture at a higher device pixel
  ratio before widening this**; the slack left on the page is deliberate
  white space, not room this block should take.

#### Page 4 — Live today · ~12s

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
  `dropset.io/currencies` does on page 3. The globe is **not**
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
  here, where it goes stale. The gap **below** the statement is the tightest in
  the deck, deliberately: the space between the sentence and the captures is
  the cheapest thing on this page to give up. It is not made symmetric with the
  kicker's gap above — a kicker belongs to the sentence under it, so it sits
  closer than the sentence sits to the content. Note only the lower gap is
  page-specific; the one above comes from the shared `Eyebrow` and is identical
  on every page that carries one, so don't go looking for a page-4 override
  of it.

#### Page 5 — Currency curation · ~10s

- **On-slide:** Dropset curates market data for all Solana-based
  currencies.
- **Visual:** **One** capture, as large as the page allows — every
  currency sorted by onchain liquidity, deepest first, with price, 24h
  change and volume, market cap, liquidity and holders.
- **Spoken:** "And alongside the swap itself, Dropset curates the market
  data for every Solana-based currency: price, twenty-four-hour change and
  volume, market cap, liquidity, holders — grouped by country, or sorted
  however you want. This is sorted by liquidity, deepest first."
- **Note:** A continuation of page 4, not a new topic — same product,
  second capability. **One table, blown up.** An earlier version put
  three tables on this page (grouped by country, sorted by liquidity, and
  the tail) and none of them could be read at that size; a group-by-country
  capture was dropped for the same reason.

#### Page 6 — The long tail · ~10s

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
  got split off page 5: the eCLOB has to arrive as the answer to a
  problem the audience has just been shown in our own data, rather than
  as a design we assert is needed. Don't fold it back into page 5 to save
  a page — the pause on this slide is the setup.

#### Page 7 — The eCLOB · ~16s

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

#### Page 8 — How we grow · ~15s

- **On-slide:** FX vaults bootstrap a public liquidity flywheel.
- **Visual:** A curve of depth growing, over the flywheel's two ends —
  **Upstream** (AUDD Digital; Loon, who issues CADC) and **Downstream**
  (Altitude, CargoBill), each group alphabetical, **with their logos**.
  Each heading sits over a rule spanning its own two tiles; each tile is
  captioned with the company and what they are to us ("Loon / CADC
  issuer", "Altitude / Banking").
- **Spoken:** "We seed the markets ourselves, the way every venue that ever
  bootstrapped its own liquidity did —
  our vaults bootstrap each book, and anyone can top them off, so the
  flywheel is public rather than ours alone. The wedge is that long tail
  of currencies: spreads are wide there, and an issuer arriving with no
  depth of their own needs a day-one liquidity partner. And it's a
  two-sided market we're already doing the customer development on.
  Upstream are the stablecoin issuers — AUDD Digital, and Loon, who
  issues CADC — who mint a currency and need it to trade. Downstream is
  the demand: Altitude in banking, CargoBill in supply chain, who need to
  buy FX to settle. Connect the two ends and the depth compounds."
- **Note on the bootstrap comparison:** the spoken track used to say "the way
  Hyperliquid did", and v3 takes the name out. It is a *positive* citation —
  a playbook being borrowed, not a rival being argued against — so it is a
  genuinely different act from the open-venue page's logo row. It still goes,
  for two reasons: global rule 5 is absolute (no competitor named anywhere,
  and the appendix files Hyperliquid under competitors), and a rule with one
  sympathetic exception is a rule that erodes. The mechanism is what
  persuades anyway, and "every venue that ever bootstrapped its own
  liquidity" says it without handing a listener a company to think about
  instead of ours. The fuller Hyperliquid answer — we borrow the
  bootstrapping, not the verticalized single-app design — stays in the
  appendix, where it belongs and where it is only ever used if asked.
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

#### Page 9 — Growth roadmap · ~12s

- **On-slide:** The path to 24/7/365 FX and beyond. Then three beats in time
  order, spanning the page, each an **action word and nothing else**:
  1. **Now** — Onboard emerging stablecoin issuers
  1. **Next** — Accrue protocol fees
  1. **Later** — Expand beyond spot
- **Visual:** The three beats as a rollout along one unbroken rule spanning
  the full page width, not a static list. Each dot sits **directly above
  its own beat's heading**.
- **Note on the action words:** each beat used to carry a two-line body
  paragraph under its headline, and the review's single most concrete note
  was to cut them. Three headlines and three paragraphs is six things to
  read on a page whose job is to be taken in at a glance; the paragraphs
  are also the part a presenter is *saying* at that moment, so printing
  them made the audience read ahead of the talk. The bodies moved into the
  spoken track intact — nothing was lost, and the page now reads in about a
  second.
  **Imperative verbs**, deliberately: "Onboard", not "We onboard". The
  timeline's `Now / Next / Later` already supplies the subject and the
  tense, so a pronoun in each headline is three words of scaffolding
  restating what the rule beneath them says. This is the deck's **one place
  imperatives are correct** — everywhere else the rule is full sentences,
  and these are labels on a timeline rather than claims.
- **Note on the dots:** their spacing is **derived from the column pitch**,
  and it has to stay that way. They were originally a flex row of three
  equal segments while the text below was `space-between` on a fixed column
  width — two different geometries, agreeing only on the first column, so
  the second dot sat 27 units left of its heading and the third 53. Both
  rows now come from one pitch, so changing the column width moves the dots
  with it.
- **Spoken:** "This is the path to 24/7/365 FX, and beyond it. Now, we
  onboard emerging stablecoin issuers: we lead the vaults that give them
  day-one liquidity, and at the same time we develop the downstream pipeline
  of the companies and users who need liquid currency swaps — those are the
  two sides of the market. Next, protocol fees accrue value: this isn't a prop
  AMM, so as additional market makers come in, quotes get tighter, volume
  follows the tighter spreads, liquidity compounds, and protocol fees accrue
  value off it. Later, the product expands beyond spot — derivatives
  make the markets themselves more efficient, and they open up business use
  cases too: treasury management, hedging B2B payment flows. Hedging isn't
  just for market makers."
- **Note:** Called a **roadmap** rather than "commercial viability" — the
  growth story is the frame, and "viability" invites the question of
  whether it is viable. The headline is a *path*, not a funding
  mechanic: an earlier draft read "each stage funds the next", which made
  the page about cashflow rather than about direction. The rollout shape
  matters — a static list reads as speculation, three beats in time order
  read as a plan. This page carries the **massive-opportunity endpoint**:
  the statement names where the path *goes*, so the audience sees what
  bootstrapping a handful of pairs is supposed to add up to — and "and
  beyond" is what carries the Later beat, so the destination isn't the end
  of the company. It is a **fragment headline**, and deliberately so (see
  the exception on global rule 1): a path is a noun,
  the page is a timeline, and every sentence form of this line
  ("our path runs to…") read as a hedge about the destination rather than as
  the destination. The **Now** beat
  is deliberately plain-spoken customer development and **names no
  company** — an earlier draft attributed the bootstrap to DASMAC, which
  put the DevCo back on a slide it has receded from. The **Next** beat has to
  say that **other market makers enter**, because that is what says this is
  not a prop AMM: anyone can quote on Dropset, so quotes compete tighter
  instead of being set by whoever owns the venue, and that competition is the
  mechanism the compounding runs on. Fees only mean something after it. That
  argument is now **entirely spoken**, since the beat prints as two words —
  so if the talk track is ever trimmed, this is the sentence to protect: a
  slide reading "Accrue protocol fees" with no prop-AMM answer beside it is
  the one place this page can be misread as a rent-extraction plan. The
  **Later** headline
  is left broad on purpose ("beyond spot") so a reader fills in their own
  derivatives thesis instead of being handed one. Name the streams in
  abstracted language, not jargon — no "fee switch".

#### Page 10 — Why FX is next · ~11s

- **On-slide:** Public liquidity is what blockchains were built for. Then
  three beats left to right, separated by the deck's chevron, each a label
  over one figure — **Memecoins**, 12M+ tokens launched on Solana ›
  **Tokenized equities**, 96% of onchain volume on Solana › **Foreign
  exchange**, Next, \$9T+ a day. The third beat is set in the **accent
  color**, tile and all: the first two are things that already happened, and
  it is the claim. Then one accent line beneath the row: "Open environments
  bootstrap new asset classes".
- **Note on the accent line:** "**bootstrap**", not "consolidate". An earlier
  draft read "Each new asset class consolidates where anyone can quote",
  which describes where the asset classes ended up — a restatement of the row
  directly above it. This says what the environment *did*: an open venue is
  what lets a new asset class get started at all, which is the training-wheels
  argument the first tile makes and the reason the third one follows. It also
  puts the environment in the subject position, so the sentence is about the
  property rather than about the assets.
- **Note on "on Solana", twice:** the page names Solana **nowhere else** —
  not in the headline, not in the accent line — so both data tiles carry it
  in their own unit line. Without it the equities tile reads as "96% of some
  unspecified whole", which is the one figure on this page an investor is
  most likely to check.
- **Note on the headline:** this is v2's thesis line, **kept**. An
  intermediate v3 draft replaced it with "New asset classes consolidate where
  liquidity is public" — which states the *pattern* rather than the
  conviction. The pattern belongs under the row, as the reading of the
  evidence, and it is now the accent line; the headline is the sentence the
  whole deck has been walking toward, and the continuous read closes on it.
- **Visual:** The three-beat row **is** the visual — a flip, in the reviewer's
  word: three frames of the same story, and the audience arrives at the third
  before the presenter says it.
- **Spoken:** "So why does FX land here, and why now? Because every new asset
  class has already consolidated on Solana, in order. First memecoins — over
  twelve million tokens launched. Call that the training wheels: it looks
  unserious, and it was the proving ground. It established that anyone could
  launch a market on this chain and anyone could trade against it, at a scale
  nothing else came close to. And it is a real business — one launchpad alone
  became the number-one DEX by daily volume across every
  chain, on a billion dollars of cumulative revenue. Then tokenized equities,
  and
  that one is a big deal: ninety-six percent of onchain tokenized-equity
  volume is on Solana, and effectively nowhere else. Real-world assets are
  going the same way — Solana added more of them in the last six months than
  in its whole history before that, and it has more holders of them than any
  other chain. So the training wheels came off. FX is next, and it's the
  biggest of them. It lands here for the
  same reason the others did: public liquidity. Anyone can quote, anyone can
  trade against it, and it composes with everything else onchain — so depth
  compounds instead of sitting still. Solana is the most money-like onchain
  environment today, and I say today deliberately: it's where this belongs
  right now because it's the fastest and the cheapest, not a commitment we're
  locked into if something better shows up. What we're building is public
  liquidity, and that's portable. That's the part the permissioned rails
  structurally can't have: gate who gets to make a market and liquidity never
  compounds, it just sits where you put it. And it runs one way — a maker who
  already has access to one of those venues can quote here and hedge there, so
  their depth reaches a public book, and nothing carries it back. Public
  liquidity is what blockchains were built for — moving money is the problem
  they were supposed to solve, and this is that."
- **Note on the rework:** this page **replaces** v2's "Why the open venue
  wins", which was a two-panel comparison — three permissioned-rail logos
  red-outlined on the left, the Dropset wordmark green-outlined on the right,
  three and four bullets under them. The reviewer's objection was structural
  rather than cosmetic: in a two-minute pitch it is a **long wind-up to a
  contrast**, and it spends that time defining the other side's position
  before arguing with it. The flip makes the same point forward — the pattern
  already happened twice, so the third is the audience's own inference — and
  it costs three words per beat instead of seven bullets. Everything the old
  page argued survives, in the talk track or the appendix; nothing is on the
  slide.
- **Note on the beats:** the order is **not** chronological trivia, it is an
  escalation in seriousness — memecoins, then equities, then FX — and it has
  to stay that way. Equities is the **pivot** — a real, regulated,
  trillion-dollar asset class that went almost entirely to one chain — and it
  is the beat that earns FX. Do not add a fourth beat; three is what a room
  can take in one glance, and RWAs (the obvious candidate) are stronger
  spoken, where their caveat fits.
- **Note on the first beat — "training wheels".** Opening on memecoins is
  deliberate and slightly risky: it is the beat an investor might read as
  unserious. The talk track therefore **names that reading and turns it**,
  in three moves, and all three have to survive editing:
  1. **Concede it.** "Call that the training wheels: it looks unserious." An
     audience is already thinking it; saying it first costs nothing and buys
     the next two sentences.
  1. **Say what it proved.** It is the **proving ground** for the page's
     actual thesis — that anyone could launch a market here and anyone could
     trade against it, at a scale nothing else came close to. That is
     *public liquidity working*, which is exactly what the headline claims
     blockchains were built for, and it is why this beat belongs on a page
     about FX at all.
  1. **Establish it is a real business.** The launchpad passed Uniswap to
     become the #1 DEX by daily volume across every chain, on \$1B+ of
     cumulative revenue. Without this, "training wheels" reads as an apology;
     with it, the beat is serious money that happened to look silly.
     Then "the training wheels came off" hands off to equities. Ducking this
     framing altogether — saying "token launches" when everyone knows
     what it means — is the one thing to avoid: it is evasive about something
     that needs no evasion, and it forfeits the strongest available evidence
     that this chain's public liquidity works.
- **Note on the figures — verify before presenting.** Every number here is
  checkable, which is the standard this page is held to, and each has a
  known failure mode. Researched 2026-08-11:
  - **96% of onchain volume** — Solana took >96% of onchain tokenized-equity
    volume in June 2026 (\$3.47B that month; \$5.77B across Q2, a quarterly
    ATH). Reported variously as 95–97% depending on window; **96 is the
    conservative round number** and June is the citable month. Source:
    `cryptobriefing.com` / `solanafloor.com` June 2026 roundups.
  - **12M+ tokens launched on Solana** — and both the "over" and the "on
    Solana" are load-bearing. The precise underlying figure is **one
    launchpad's** 11.9M cumulative launches since January 2024, not a
    chain-wide count. Printing "11.9M" would state a one-platform number as
    though it were the chain's — precise, and wrong. That launchpad is only
    ~71% of Solana's daily token creation and other launchpads exist, so the
    chain's real total sits *above* it: "over 12M" is a **floor**, which is
    imprecise and true, and a skeptic can only discover the real figure is
    larger. Being wrong in that direction is the point. Source: launchpad's
    own reported total, June 2026.
  - **Cut, and worth knowing why:** a **cross-chain launch comparison**
    ("more token launches than any other chain"). It is widely repeated and
    probably true, and five searches could not source it to a current,
    citable dashboard — only qualitative assertions. It is therefore on
    **neither** the slide nor the talk track. The cross-chain point is made
    instead by a fact that *is* verified: the launchpad became the **#1 DEX
    by 24h volume across all chains** (~\$1.77B, July 2026, overtaking the
    largest Ethereum-based venue), on **\$1B+ cumulative revenue** (March
    2026). That carries the "Solana beat everyone else at this" work without
    a superlative nobody can source — and it is a *trading* fact, which is
    better evidence for a page arguing about liquidity than a creation count.
    Note the spoken line says "**that one launchpad**" and names neither it
    nor the venue it overtook: both are DEXes the appendix files under
    competitors, and global rule 5 admits no sympathetic exceptions (the same
    call that took Hyperliquid out of page 8). "#1 across every chain" makes
    the point without a name, since overtaking everyone includes them.
  - **RWA momentum** — spoken only, and worded as *growth*, never as size.
    Solana added ~\$2B of tokenized RWAs in six months (~\$1.4B → ~\$3.6B at
    the early-July ATH), took the highest 30-day net inflows of any chain
    (~\$967M), and has the most RWA holders (300k+). It is **third by total
    RWA value**, behind Ethereum (~\$15.9B) and BNB (~\$3.9B) — so any
    on-slide "Solana leads RWAs" claim is simply false and hands a listening
    investor a free correction. Source: `rwa.xyz`.
  - **Cut deliberately:** the "99% of tokens are created on Solana" framing
    from the review notes. No source supports it, and the weaker
    "more new tokens than every other chain combined" could not be verified
    against a current dashboard either. It is out of both the slide and the
    talk track — an unverifiable number next to two verified ones is what
    makes an audience doubt all three.
- **Note on competitors:** **no competitor is named or shown, on this page
  or anywhere in the deck** — not on the slide, not in the `Notes`. This
  **reverses** v2's global rule 5, which sanctioned exactly one
  red-outlined logo row here. Two reasons the review gave, both good: a
  two-minute pitch shouldn't spend its scarcest seconds giving three
  competitors free press, and naming them invites the audience to
  evaluate *them* rather than us. The spoken track says "**permissioned
  rails**" or "the other guys" and never more. The substantive argument
  is unchanged and is made as a **property claim** — gated access can't
  compound liquidity — which is stronger than an attack because it is
  about mechanism rather than about a company.
- **Note on absorption:** the one-way claim survives the rework and stays
  **spoken**: a maker who already holds access to a gated venue can quote
  here and hedge there, so their depth reaches a public book, and nothing
  carries it back. It repairs a soft spot on page 8, which otherwise leaves
  our own vaults as the only answer to where depth for a thin pair comes
  from. Say it as a **capability**, not as something running today — it
  depends on how a given maker is integrated. The "vampire attack" phrasing
  is retired from the deck entirely: with no competitor named, an attack
  framing has no referent, and the term properly describes poaching an
  incumbent's liquidity providers with incentives, which is a different
  mechanic.
- **Note on the mechanism line:** the accent line under the row ("Open
  environments bootstrap new asset classes") is what keeps the page from
  being three statistics and a hope. The beats are evidence of a pattern;
  this is the reason the pattern extends to FX. Keep it to **one line** — it
  is the page's whole spend on mechanism, and a second line turns the flip
  back into a wind-up.
- **Note — the flywheel graphic is a deliberate won't-do.** The filed task
  asked this page to carry "a graphic around the flywheel of permissionless
  open liquidity", riffing on page 8's SVG. Three treatments were put up
  (the chevron row; a flywheel ring as the hero image with the asset classes
  feeding it; both stacked) and the **chevron row was chosen**, in session,
  on 2026-08-11. Two reasons it wins: a cycle diagram and a three-beat
  progression compete for the same attention, and the review's primary
  complaint about this page was that it was *too long a wind-up* — a second
  visual is the one thing guaranteed to make that worse. The flywheel
  language survives where it is already earning its keep, on page 8. Revisit
  only if this page ever loses the row.
- **Note on the subtext line's polarity.** The filed task asked for
  "permissioned falls short" as the one subtext line. It is **positive**
  instead — the accent line says what open environments *do*, and the
  permissioned-rails point is spoken. Naming the shortfall on-slide would
  have reintroduced the argue-against-them framing the same task was
  removing, and global rule 5 leaves it nothing to name.

#### Page 11 — Team · ~8s

- **On-slide:** Eyebrow "The team", then "Dropset is built by people who
  have built exchanges" — matching every other page's kicker-plus-sentence
  shape. Then one line each: Alex Kahn, Founder — authored exchange
  technology across the entire stack on multiple blockchains, including the
  Econia order book on Aptos (\$500M lifetime volume) and the Solana Opcode
  Guide, a key resource for optimizing Solana program efficiency.
  Judy Sosa, Operations — owns the whole operational stack, working with
  banks, stablecoin providers, onramps and service providers, on an extensive
  background in logistical coordination and partner relationship management.
  Roles carry **no company**: "Founder", not "Founder, DASMAC" — the deck
  names the company once, in the footer. The Opcode Guide is "**a key**
  resource", not "the definitive" one — the superlative is a claim a reader
  can dispute, and the page reads stronger without one to argue with.
- **Visual:** Both headshots, square and unframed, pulled from the
  marketing site at build time (`remote-assets.json`).
- **Spoken:** "Dropset is built by people who have built exchanges. I've
  authored exchange technology across the entire stack, on more than one
  blockchain — the Econia order book on Aptos, five hundred million dollars
  of lifetime volume, and the Solana Opcode Guide, a key resource
  for optimizing Solana program efficiency, which is what makes quoting on
  the eCLOB cost double-digit compute units. Judy owns the whole
  operational stack, and works directly with banks, stablecoin providers,
  onramps and service providers, on an extensive background in logistical
  coordination and partner relationship management."
- **Note:** **State what each person has done; don't argue for why the
  role matters.** An intermediate draft justified the operations split
  ("this is the work that gets an FX venue integrated with the rails…",
  "a dedicated owner rather than a founder's side task") — that reads as
  defending the team, and it framed one person's work relative to the
  other's rather than on its own terms. One sentence each, both in the
  same voice. The credential reads "Dragonfly Capital", not "…Partners",
  with the EA role stated plainly. This page **no longer closes the deck** —
  page 12 does — so the mirror-the-title line moved there and the
  "[Leave this page up]" direction went with it. Longer copy is still
  correct here (two bios is the page), but it is no longer the page an
  audience stares at for the rest of the session.
- **Note on the block:** set name, role and prior as **one tight unit** — no
  margins between them at all, their leading is the only separation — so the
  only gaps that read as gaps are headshot-to-name and the white space before
  the bio. This is the deck's second densest page after page 4, thanks to two
  five-line bios under two headshots, so the gap above the portraits is
  tightened as well.

#### Page 12 — Close · ~4s

- **On-slide:** The Dropset wordmark and the tagline — The liquidity layer for
  every national currency. A replay of page 1, and nothing else.
- **Visual:** Identical to the title page, deliberately.
- **Spoken:** "Dropset — the liquidity layer for every national currency."
  Then the personal why (below), which is the last thing the room hears.
- **Note on why this page exists:** the review's sharpest note was that it
  was left **wondering why we care about this**, and that people invest in a
  founder at least as much as in a problem. v2 ended on the team page, which
  states credentials and answers "can they build it" — not "why are they the
  ones who will". This page is the room to say that, and putting it on a
  replay of the title means the why-me lands over the deck's own thesis
  rather than over two headshots. It also gives the talk a **bookend**:
  the tagline is the first thing said and the last, which is the oldest
  trick there is for making a two-minute pitch feel composed rather than
  rushed.
- **Note on the personal why — a rehearsal draft, in the founder's own
  argument.** This is the one block in the spec that is **not** finished copy:
  the beats are the real ones, and the wording is meant to be iterated on out
  loud rather than read out. The beats, in order:
  1. **The seventeen-year frame — the industry's age, not the speaker's
     tenure.** This industry is seventeen years old, and sending money around
     the world is the part that works. Getting in and out of currencies is
     the part that doesn't.
  1. **What did get solved, as the contrast.** There is no shortage of ways
     to speculate onchain and no shortage of alternative stores of value; but
     the currencies people actually earn and spend in don't flow on a
     decentralized ledger the way it promised they would.
  1. **Why FX specifically, and why here.** A large part of why is that there
     is no FX liquidity onchain — specifically on Solana, which is a deeply
     money-like environment, and so the place this gets fixed.
     Keep it to **two or three sentences out loud**. The failure mode is a
     biography; the target is the single sentence that makes a listener believe
     this person would still be working on this in five years.
- **Note on the layout:** it reuses the title page's components exactly —
  same wordmark width, same statement size, same centred body. Reusing
  rather than rebuilding is deliberate: the replay only works if the page is
  *identical*, and two hand-built pages drift the moment one is edited.

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

**No competitor appears anywhere in the deck** — not on a slide, not in a
spoken track. These are the answers to have ready **if asked**, and asked is
the only way they come out. Page 10 makes the argument as a property of
public liquidity instead, which is why these can be this blunt: nothing here
has to be said in front of a room.

- **The settlement chains (Arc, Tempo) and regulated onchain markets
  (Canton).** Off-slide now, along with everyone else — v2 showed these
  three red-outlined and the sign-off review cut them. Each is chasing
  onchain settlement
  and each arrives with customers already on it. The answer is **not** that
  an issuer would never go there — an issuer needs distribution, and a
  private rail can supply it. It's that the rail's own business dynamics
  cap what can ever be built on it: settling on a competitor's ledger
  carries competitive friction, so the venue can never be genuinely neutral,
  and a new entrant meets a business account and gated access before it
  meets any liquidity.
  The moment FX needs a *neutral* venue where anyone can make a market and
  anyone can trade, a closed garden can't serve it. And the relationship is
  **asymmetric in our favor**: a maker who already holds an account on one of
  those rails — or on an offshore venue like Binance, which is where a lot of
  FX hedging actually happens — can quote here and hedge there, so their depth
  reaches a public book, while nothing moves the other way. Whoever holds the
  gated access is the pipe, and the private venue cannot be the pipe in
  reverse. That is why public liquidity ends up the superset. The blunt way to
  say it in a technical room is that it's a vampire attack that only runs one
  direction; keep that out of print.
- **The existing Solana DEXes (Jupiter, Meteora, Orca, pump.fun,
  Raydium).** They aren't focused on FX, and we're beating
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
- "How do you make money?" → Page 9 is the answer, and the appendix
  detail is that each stage compounds into the next: liquidity
  operations now, protocol fees as the books thicken and volumes
  compound, derivatives once there's enough depth to hedge against.
- "Isn't 'FX is next' just an assertion?" → Page 10's beats are the
  evidence that the pattern is real (memecoins, then equities at 96% of
  onchain volume), and the mechanism is why it extends: FX needs a neutral
  venue where anyone can quote, which is the property those asset classes
  came here for. The honest limit is that it hasn't happened yet — which is
  the investment.
- "Why hasn't a big DEX just done FX?" → The DEX answer above, said out
  loud: their customer is the retail speculator, the volume today is too
  small to move a giant, and we'll be here for the next 10x.

### Figure sources

Every number on page 10 traces to a citable source, so a "where's that
from?" is answered with a link rather than a shrug. Researched
**2026-08-11**, corroborated independently by a second pass the same day.

Two standing caveats, and they apply to **every** entry below:

- These were gathered from **search results, not by reading each page end
  to end**. Open the source and confirm the exact figure before it is
  spoken to investors.
- Several are **current-state** numbers that move (share percentages,
  rankings, holder counts). **Re-check before presenting.**

**Tokenized equities — 96% of onchain volume on Solana.** Reported at
95–97% depending on the window; 96 is the conservative round number, and
June 2026 is the citable month (\$3.47B that month; \$5.77B across Q2, a
quarterly high; ~\$4.9B across H1).

- <https://cryptobriefing.com/solana-tokenized-stocks-volume-surges-h1-2026/>
- Cryptobriefing, "Solana hits \$3B in tokenized equities volume for June
  2026, leads market" — the ">96% of that month's global onchain
  tokenized-equity activity" phrasing, and the best single citation for the
  headline number
- Yahoo Finance / Genfinity, "Solana hits \$5.77B tokenized asset volume in
  Q2 2026" — the quarterly ATH
- xStocks specifically, if a conversation goes there:
  <https://solana.com/news/case-study-xstocks> (Solana's own case study),
  plus Kraken's "\$25 billion in total transaction volume" xStocks post

**Do not cite the per-chain split** (Solana ~95.6% / Gnosis ~2% /
Ethereum ~1.8%). The only source found for those exact decimals was a
low-quality aggregator. Either trace it to its upstream (a Dune or
`rwa.xyz` dashboard) or drop the decimals and lean on the ">96% of June
volume" phrasing above, which is properly sourced.

**Memecoins — 12M+ tokens launched on Solana.** The underlying figure is
one launchpad's 11.9M cumulative launches since January 2024, which is
~71% of Solana's daily token creation — hence the deliberate "over 12M"
floor (see the page-10 figures note).

- <https://en.cryptonomist.ch/2026/06/10/pump-fun-solana-token-launches/>
- <https://cryptobriefing.com/solana-42000-tokens-pumpfun-launch/>
- <https://defillama.com/protocol/pump>

**The launchpad as a real business** — #1 DEX by 24h volume across all
chains (~\$1.77B, July 2026), \$1B+ cumulative revenue (March 2026). Spoken
only, and **named neither on the slide nor in the talk track** per global
rule 5.

- <https://cryptobriefing.com/pumpfun-top-dex-surpasses-uniswap/>

**This pair is the weakest-sourced claim in the deck** — a single
secondary article, and a second research pass found no corroboration for
either figure. Both have checkable homes: DeFiLlama
(<https://defillama.com/protocol/pump>) for cumulative volume and revenue,
and the launchpad's Dune dashboards for the DEX ranking. **Confirm both
there before saying them out loud**, and drop the claim rather than soften
it if either fails to check out — the training-wheels beat still works
without it, on the token-launch floor alone.

**RWA momentum** — spoken only, and worded as *growth* rather than size:
~\$1.4B → ~\$3.6B in six months, highest 30-day net inflows of any chain
(~\$967M), most RWA holders (300k+). Solana is **third by total RWA
value**, so never claim it leads on size.

- <https://solanafloor.com/news/solanas-rwa-market-hits-record-3-62-b>
- <https://cryptobriefing.com/solana-rwa-market-4-billion-growth/>
- <https://app.rwa.xyz/networks/solana>

**Not sourced, and therefore not used:** a cross-chain launch comparison
("more token launches than any other chain"). Widely repeated, probably
true, and two independent research passes found no current citable
dashboard — only qualitative assertions and aggregator copies. It is on
neither the slide nor the talk track.

The closest real citation is The Block, "Solana saw nearly half a million
tokens launched last month" — **455k on Solana vs 177k on Base and 39k on
BNB**. Two problems, both fatal for a slide: it is **May 2024**, two years
stale, and those three figures do not establish "more than every other
chain combined" without the rest of the tail. Recorded here so the next
person does not re-run the same five searches, and does not mistake it for
a usable citation. The daily-launch figures floating around
("25k+/day on Solana launchpads") appear only in low-quality aggregators —
talk-track color at best, never a slide.

______________________________________________________________________

## 3. Formatting / structure rules

### How to read this

- **One page = one slide.** Twelve pages (see "Format rules" for why the
  ten-page cap moved, and what it costs).

- Each page gives: the **on-slide line** (what the audience reads), the
  **visual** (the one big image), the **spoken copy** (what the
  presenter says — this is the real script), and a **time** budget.

- Total spoken time targets **~120 seconds**, and the per-page budgets sum
  to exactly that. The v3 changes **fund each other** — the two new pages
  buy their seconds from pages that got shorter, so the total is unchanged:

  - the gap split **costs** +2 (v2's single 15s gap page becomes 7 + 10);
  - the closing page **costs** +4 (new);
  - the roadmap **saves** 3 (15 → 12) with its paragraphs off the slide;
  - the eCLOB **saves** 2 (18 → 16);
  - the flip page **saves** 1 (12 → 11) over the open-venue page.

  That is +6 against −6. **If a page grows, take the time from another
  page** — the two-minute cap is the accelerator's, not ours.

- Every page carries the same footer: the Dropset wordmark at the left,
  the "Built by DASMAC" credit in the middle, and progress dots at the
  right. It isn't page content — don't budget words or space for it; the
  slide body already reserves room for it. That reserve is **split across the
  top and bottom of the body**, not taken off the bottom alone: content is
  centered in what's left, so reserving at one end only puts the whole page's
  content ~53 units above the slide's own center, which reads as an eyebrow
  crowding the top edge with an empty band under the content. Splitting it
  costs no page any height. The credit's own centering is **two mechanisms,
  not one**, and nothing ends up sitting exactly on the midline: "Built by" is
  small grey text that pads the lockup's left while contributing almost none
  of its ink, so **neither** part has a geometric centre that also looks
  centered — put the lockup's box on the midline and the mark reads ~60 units
  right of it; put the mark on the midline and the lockup reads ~50 units
  left. The perceived centre is between them, nearer the box-centered end. So
  the label is rendered twice — the second copy hidden on the mark's right,
  making the row symmetric about the mark, which is an exact reference point —
  and then one named constant walks the pair off that point to where it looks
  right. Structure holds the geometry, one number holds the judgement. Both
  extremes were measured off screenshots rather than judged by eye; the deck
  comment records those numbers, the constant's usable range, and the two
  traps met along the way (a theme-scale margin, and losing flex centering by
  positioning the label out of flow).

- Presenter mode is **`⌘⇧P`** (`Ctrl⇧P` off macOS), not a bare `p`.

- Anything nuanced — the fuller competitor answers, the investor
  grilling, the numbers behind a claim — is **not on a slide**. It lives
  in the appendices (section 2) and only comes out if a conversation goes
  there.

### Global rules — v3

These are firm, and they override the older guidance where the two
disagree:

1. **Full sentences on-slide**, everywhere. No fragments — not as
   headlines, not as list items. A reviewer reading the deck without
   the talk should get the argument. **Three exceptions**, all of them
   naming rather than claiming:
   - the **title and close** (pages 1 and 12), "The liquidity layer for
     every national currency" — a tagline is a name for the company, and the
     sentence forms of it ("Dropset is the…") put a subject on a page whose
     subject is the wordmark directly above the line;
   - the **roadmap headline** (page 9), "The path to 24/7/365 FX and
     beyond" — the page is a timeline and its headline names a
     destination, which every sentence form of turned into a hedge about
     reaching it;
   - the **roadmap's three beats** (page 9), "Onboard emerging stablecoin
     issuers" and its two siblings — imperatives, where the timeline's
     `Now / Next / Later` already carries the subject and the tense.
     This exception is new in v3 and it is *why* the beats are short
     enough to have replaced three body paragraphs;
   - the **flip tiles** (page 10) — a label, a figure, and its unit
     ("Memecoins / 12M+ / tokens launched on Solana"). These are
     **measurements, not claims**: the sentence that reads them is the
     accent line under the row, and the headline above it is the argument.
     Writing them as sentences ("Twelve million tokens have launched on
     Solana") would turn three data points a room can take in at a glance
     back into the prose block this rework removed.
1. **No terminal period on a headline — on any page, at any level.** At
   display size a full stop is a visible mark that earns nothing: there is
   no following sentence for it to separate. Sentence *structure* still
   applies (rule 1); only the period goes. This covers **every** page
   headline, the footer's "Built by DASMAC" credit, **the roadmap's three
   beat headlines**, and **page 10's beat labels and figures** — anything
   that reads as a title or a list item rather than as prose.
   Multi-sentence copy — the team bios — keeps its punctuation, because
   there the period is doing its actual job.
1. **16:9 aspect ratio**, set explicitly on the deck rather than
   inherited.
1. **Static images only.** No embedded video, no gifs, no player. A
   product beat is an interface screenshot with a claim over it. This
   retires the click-to-play badge and the two recorded demos.
1. **No competitor appears in the deck — no logo, no name, on any slide
   and in any spoken track.** This **reverses** v2's rule, which allowed
   exactly one red-outlined competitor row on the open-venue page. A
   two-minute pitch cannot afford to spend its scarcest seconds giving
   competitors free press, and naming them invites a room to evaluate them
   instead of us. Spoken, they are "permissioned rails" or "the other
   guys". The only marks left in the deck are **ours** and our **partners'**
   (page 8), and a partner mark is still captioned with what the company is
   to us — a logo is argued, never listed. The fuller competitor answers
   live in the appendix and come out only if someone asks.
1. **Solana is never framed as a ceiling.** It's the deliberate start —
   the most money-like environment onchain, with the highest ease of
   transmission — never the boundary. The on-slide carrier is page 3's
   "the fastest and cheapest chain", which names what Solana is best *at*
   rather than treating the choice as given. The **"today"** qualifier —
   v2's second carrier, on the open-venue page — is now **spoken only**,
   since the page that printed it is gone. It still has to be said: it is
   the answer to *what if a better settlement layer arrives*, and nothing
   here is bound to this one, because what's being built is public
   liquidity and that is portable.
1. **Dropset is the brand; DASMAC recedes.** The company is the boring
   DevCo in the background — someone finds it when they sign a document.
   The deck names it exactly **once**, in the footer credit: not on the
   title slide, not in the roadmap's beats, not in the team's roles. This
   reverses the earlier rule that asked for the company/protocol
   distinction to be legible on the slides; carrying it cost attention the
   product needed, and a title slide arguing for a DevCo argues for the
   wrong thing.
1. **No bullet lists — no exceptions.** v2 sanctioned two (the gap page's
   six chevron facts and the open-venue page's panel bullets); v3 has
   neither, because the pages that carried them were split and replaced.
   Page 10's three beats are a **row of peers**, not a list: they read
   left to right as a sequence, each is two or three words, and the
   chevrons between them mark progression rather than enumeration. If a
   page ever wants a bullet list again, that is the signal it is carrying
   an argument that belongs in the talk track.

### Brand

The Kargil Studios design system from the DASMAC Figma:

- **Typography.** **Inter** is the primary family, and it carries
  everything in **sentence case** — which is the reason it's primary
  rather than the mono face. **Space Mono** is the mono/tag face, set
  **uppercase and letterspaced**, which is exactly the treatment the
  company banner used for its own tag. So the deck's kickers and the
  company's own typography are visibly one system. This supersedes an
  earlier note naming JetBrains Mono. Both are Google fonts loaded through
  `next/font`, so there are no font files to commit.
- **Assets.** `brand-assets/` at the repo root holds the DASMAC and
  Dropset wordmarks and the favicon, all copied into `public/` on the
  `predev` / `prebuild` hooks. The wide DASMAC company banner used to live
  there for the title slide; with the title slide down to the Dropset
  wordmark alone, the banner has no consumer and was deleted rather than
  left as an orphan for the copy step to keep shipping.

### Export

The deck runs on Spectacle, and it stays there — there is no migration
and no export tooling to build. The only requirement this puts on the deck
is that **every slide prints as a clean 16:9 static page** — nothing
clipped, no interactive chrome in the output.

**Use export mode (`⌘⇧E`), not print mode (`⌘⇧R`).** They render the same
thing — every slide stacked as a static page — with one difference that
matters enormously here: print mode merges Spectacle's own **print theme**,
which forces the backdrop to white, headings to black and body text to
`#777`. On a deck designed on black that inverts the whole thing, and it
**erases the Dropset wordmark**, because that asset is an opaque PNG shown
with `mix-blend-mode: screen` and screening over white returns white. Export
mode skips the theme swap and keeps the deck's real colors. (An earlier
version of this section said `⌘⇧R`; it was wrong.) Both modes are also
reachable by query string — `?exportMode=true`, `?printMode=true` — which is
the more reliable way in, since `⌘⇧R` is "hard reload" in a browser.

Then, in the print dialog:

- Destination **Save as PDF**.
- **Background graphics ON.** Without it the black backdrop and the panel
  tints don't print, and the deck comes out on white anyway.
- Margins **None**, scale **100%**, headers and footers **off**.
- Leave the paper size alone. Spectacle emits
  `@page { size: <deck width>px <deck height>px }` from the theme's own
  `size`, so pages come out exactly 16:9 — check the preview for
  edge-to-edge black with no white bands.

**Google Slides can't import a PDF** (File ▸ Import slides takes `.pptx` /
`.ppt` / Slides only), so the meta-deck path is: PDF → one image per page →
insert into a 16:9 Slides deck, each image filling the canvas. Rasterizing
needs a tool the repo doesn't carry — `pdftoppm` from poppler, or PyMuPDF.
Ask the accelerator whether a PDF is acceptable first; it usually is, and it
skips this step entirely.

Content that overflows a slide is merely scaled on screen but **silently
clipped in the exported page**, so a layout change means one pass through
export mode before it's done.

**Two traps make a page taller than its margins say.** Both were found while
chasing exactly that symptom, and both cost tens of units per page:

- **A unitless `0` margin is not zero.** Spectacle's components resolve
  `margin` through styled-system, which looks a *unitless* value up in the
  theme's `space` scale — so `margin="0"` means `space[0]`, which is **16
  units**. The page headline carried that on all four sides for a long time,
  which is ~32 units of vertical space per page that no margin in the file
  appeared to control. Always write the unit: `margin="0px"`.
- **Nothing sets a line-height.** `Text` and `Heading` inherit the browser's
  `normal`, ~1.21 for Inter, so a 38px line hangs ~19 units of leading around
  a ~27-unit cap height. Stack three of those (the team page's name / role /
  prior) and the leading, not the margins, is most of the spacing — trimming
  margins there barely moves anything. Set `lineHeight` explicitly on
  single-line display type; leave multi-line prose alone, where 1.21 is
  already tight.

An eyebrow sitting *close* to the top edge is a different symptom from one
that is **cut off**, and it has a different cause: the page is dense enough
that centering leaves it little room, which is only comfortable because the
footer reserve is split across both ends of the slide body (see the footer
note in "How to read this"). Take height out of the page if it still crowds;
don't re-balance the reserve, which would cost every page height.

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

Note where the deck **departs** from this: "one big sentence per page"
still holds, but full sentences replaced fragments everywhere, and page 11
carries a line per person. A reviewer reading only the deck is a
first-class case.

The **page cap** is the one departure v3 makes knowingly, and it is worth
stating plainly rather than quietly: this advice says ten pages and the
deck is twelve. It goes over because the accelerator's own sign-off review
asked for two specific splits — a two-beat open and a closing page — and
both make the deck *faster* to read, which is what the cap is a proxy for.
Twelve pages at ~120 seconds is ten seconds a page; the v2 gap page alone
took fifteen. **The real constraint is the two minutes, not the page
count**, and the deck holds it. Adding a thirteenth page still has to
displace something.

#### Name why it will fail, then answer the counters

Put up the honest risk and don't flinch, then show it's been thought
through — surface the lazy-VC questions and answer them, and be ready to
reply to the counters rather than hoping they don't come up. An investor
respects that the risk was named and met with an answer.

In v1 this was a two-page setup-and-payoff pair with an asterisk gag.
In v2 it was one page — the pain point and the answer in the same breath.
In **v3 it is off the slides entirely**: the risk is real and the answers
are sharp, but a two-minute pitch that argues against an objection has to
raise the objection first, and the review's verdict was that this cost
more than it returned. The steelman versions and the replies live in the
appendix, where they come out **only if asked** — which is where this
advice puts "nuanced thoughts" in the first place.

### Format rules (distilled from the above)

1. **Twelve pages, and the budget is time.** The children's-book advice
   says ten; this deck is twelve at ~120 seconds, because the accelerator's
   own review asked for the two-beat open and the closing replay. Anything
   added from here has to displace something — **seconds**, not pages.
1. **One big sentence per page.** No bullet lists, anywhere.
1. **One big image per page.** Name the image in the "visual" field. A
   page may show several captures of *the same thing* (the swap flow, the
   maker and the frontend it feeds) — that is still one image in the
   sense that matters, because it's one idea. It does **not** permit three
   unrelated tables on a page; that was tried and none of them could be
   read.
1. **The sentences tell a story as you flip through.** Read the on-slide
   lines top to bottom and they should read as one arc. Pages
   4–7 are the load-bearing stretch: works today → we curate the data →
   the tail is empty → here's the exchange that fixes it.
1. **Super simple words.**
1. **Lead with the strongest selling point, not a template.** It already
   works, on mainnet, and there's a screenshot — so "live today" is page
   4 and the arc is built around it.
1. **No market-opportunity slide.** FX size appears once, as the shape
   of the gap, never as a trillion-dollar brag. Page 2 is a single number
   for exactly this reason: it is one beat of an open, not a TAM page, and
   the moment it acquires a breakdown or a growth rate it becomes one.

### Reference — the accelerator's 7-point pitch structure

The Colosseum "basic pitch" framework, from the pitch review in the
fundraise tracker. Not the deck's structure — the children's-book arc
wins for a 2-minute demo — but every point below must be *covered*
somewhere, and this is the checklist the accelerator expects. Mapping to
our pages in brackets.

1. **One-liner.** DASMAC is building Dropset, an onchain Forex platform
   that harnesses Solana for open, efficient exchange of multinational
   currencies at scale. [Pages 1, 12]
1. **Problem / unique insight.** ~14 currencies now live on Solana via
   stablecoins; Solana settlement can support the massive FX market
   *composably* — DevEx convenience for payments providers, merchants,
   manufacturers, and retail — because Solana is general-purpose, not
   verticalized like Hyperliquid. [Pages 3, 6, 10; appendix]
1. **Solution / product.** Dropset routes existing onchain liquidity
   through aggregators and adds a novel eCLOB to bootstrap new markets
   with inexpensive quote updates that accelerate market-maker
   onboarding. [Pages 4, 7]
1. **Traction.** Dropset.io is live and clearing trades on mainnet
   (today via aggregators), and curates the market data for every
   currency on Solana. [Pages 4, 5]
1. **Why the market is massive.** FX is >\$9T/day and 24/5; Solana as
   intermediary gives atomic settlement and faster on/off-ramps. \[Page
   2\]
1. **Why now.** The non-US stablecoin market has only just started to
   expand — EUR stablecoins drive most volume, more currencies going
   live (14 on Solana). Page 10 is the other half of the answer: the
   asset classes that already consolidated onchain are the pattern FX
   follows. [Pages 3, 5, 10]
1. **Business model.** Liquidity operations now, protocol fees as
   volumes compound next, derivatives after that. [Page 9]
1. **Founders' bio.** Exchange-design background — authored the Econia
   order book (~\$500M on Aptos) and the Solana Opcode Guide — with a
   dedicated operations owner on banking and accounting. Full detail on
   page 11 and in the appendix (kept there to stay DRY). Page 12 carries
   the **why-me** beat, which this checklist doesn't ask for and the
   review did. [Pages 11, 12; appendix]
