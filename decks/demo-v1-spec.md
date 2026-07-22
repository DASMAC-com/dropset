<!-- cspell:word Econia -->
<!-- cspell:word Hibachi -->
<!-- cspell:word Cargobill -->
<!-- cspell:word Dragonfly -->
<!-- cspell:word Sealevel -->
<!-- cspell:word onramps -->

# Demo-day pitch spec — `demo-v1`

The **copy** for the ~2-minute demo-day pitch, written to be reviewed
and edited *before* it's turned into slides. This is the script and
page plan; the built deck lives at `app/demo-v1/` and should follow
this doc, not the other way around. Drop it into Google Docs, get
edits from others, then reconcile the deck to match.

## How to read this

- **One page = one slide.** Ten pages, hard cap (see "Format rules").
- Each page gives: the **on-slide line** (the single big sentence the
  audience reads), the **visual** (the one big image), the **spoken
  copy** (what the presenter says — this is the real script), and a
  **time** budget.
- Total spoken time targets **~120 seconds**. The two live demos eat
  ~50s of that, so every other page has to be fast.
- Anything nuanced — the moat argument, the investor grilling, the
  numbers behind a claim — is **not on a slide**. It lives in the
  Appendix and only comes out if a conversation goes there. (See
  "Format rules" for why.)

## Guidelines — the principles this deck is designed to

Two pieces of outside advice are the priority for this deck. They're
quoted here so reviewers edit against the same rules, not just taste.

### Mert — "design it like a children's book"

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

### Clay — name why it will fail, then answer the counters

Put up a **"why this will fail"** beat and don't flinch: say the honest
version an investor is already thinking ("Arc, Stripe, and Tempo are
good enough; open access isn't actually important"). Then show you've
thought it through — **surface the lazy-VC questions and answer them**,
and be ready to **reply to the counters** rather than hoping they don't
come up. An investor respects that the risk was named first and met
with an answer. The honest threat is Page 7; the answer is Page 8; the
fuller counters-and-replies live in the Appendix.

## Format rules (distilled from the above)

Design it like a children's book, because a demo-day audience scrolls
it like Twitter — they will not read a word salad.

1. **Max 10 pages.** This deck is exactly 10.
1. **One big sentence per page.** No bullet lists. Where the current
   built deck uses three-bullet lists (pages 2, 3, 6), this spec
   collapses each to a single line — the deck should follow.
1. **One big image per page.** Name the image in the "visual" field.
1. **The sentences tell a story as you flip through.** Read the ten
   on-slide lines top to bottom and they should read as one arc.
1. **Super simple words.**
1. **Lead with the strongest selling point, not a template.** The
   strongest point here is *it's live and I can trade it in front of
   you* — so the demo comes early and the arc is built around it. No
   generic problem → solution → market-size structure.
1. **No market-opportunity slide.** We do not put up a "$9T TAM"
   slide. FX size appears once, as the *shape of the gap* ("the
   biggest market on earth barely exists onchain"), never as a
   trillion-dollar brag.

## The 2-minute narrative (continuous read)

The through-line, so the story reads as one piece before it's cut into
pages:

> Dropset is onchain Forex on Solana. The biggest market in the world
> — nine trillion dollars a day of currency — barely exists onchain
> and has no liquid home. So we built the eCLOB: the depth of an order
> book, with quote updates as cheap as an AMM. It's live on mainnet
> today — here, watch me swap euros for dollars right now. And here's a
> brand-new market coming alive from nothing: empty book, I turn the
> makers on, and real depth fills in within seconds. Which means right
> now, during this demo, my 2021 laptop is the most liquid place to
> trade these currencies onchain. Why won't this work? Arc, Tempo,
> Hibachi, Canton are all chasing onchain settlement. But they're
> private, permissioned, or walled gardens — Dropset is the open,
> neutral venue anyone can quote on and anyone can trade against. We
> bootstrap the liquidity ourselves the way Hyperliquid did, and
> Colosseum partners like Altitude and Cargobill already need to source
> FX onchain. The team has built exchanges before — I've shipped two
> exchanges and an onchain order book, and Judy runs the operations
> that get us banked. Dropset — onchain Forex, on Solana.

## Page-by-page

### Page 1 — Title · ~5s

- **On-slide:** Dropset — onchain Forex, on Solana.
- **Visual:** Dropset wordmark, centered, on the dark theme.
- **Spoken:** "Dropset is onchain Forex on Solana — currencies,
  exchanged at scale."

### Page 2 — The gap · ~12s

- **On-slide:** The biggest market on earth barely exists onchain.
- **Visual:** A nearly-empty order book / a world map of currencies
  with almost none lit up. (One image — not a stat table.)
- **Spoken:** "Foreign exchange is nine trillion dollars a day, trading
  around the clock. But onchain, the top FX currencies have no liquid
  home — the market is only just opening."
- **Note:** This is the one place FX size is mentioned. Frame it as the
  *gap*, said out loud, never as a market-size slide (per Mert).

### Page 3 — The eCLOB · ~15s

- **On-slide:** So we built the eCLOB: order-book depth, AMM-cheap
  quotes.
- **Visual:** A clean order-book ladder (the TUI/frontend book view).
- **Spoken:** "Our edge is a new exchange design — the eCLOB. You get
  the liquidity guarantees of a central limit order book, but quote
  updates as cheap as a proportional AMM. That lets us source liquidity
  where there's almost none, and onboard market makers fast."

### Page 4 — Live on mainnet [DEMO] · ~25s

- **On-slide:** Live on mainnet today — watch me trade the euro right
  now.
- **Visual:** The frontend swap screen, mid-swap. Slide shows the
  command **`make demo`**.
- **Spoken:** "This isn't a prototype. Dropset is live on mainnet,
  clearing real trades. Here I'll swap EURC for USDC on the live
  frontend. [run the swap] — that just settled on mainnet."
- **Demo ops:** Mainnet swap is the primary. If mainnet is unreachable
  (rate-limit, inventory, key), fall back to the localnet demo on Page
  5 as the live proof, then to the recorded video. See Appendix →
  "Demo failure modes."

### Page 5 — A market comes alive [DEMO] · ~25s

- **On-slide:** Empty book, makers on — real depth in seconds.
- **Visual:** Split view — the maker TUI on one side, the frontend
  order book on the other, book filling from empty. Slide shows
  **`make demo`**.
- **Spoken:** "To show how a book is born, here's the same stack on
  localnet from an empty market. I turn the maker bots on — watch
  top-of-book fill in live — then I trade against real eCLOB depth and
  it fills the size. This is the market maker's view; the frontend is
  the user's view."
- **Note:** This local demo is a required beat — it's the clearest
  proof that we can *manufacture* liquidity, not just display it.
  Optional flourish if time allows: from the TUI, reshape the ladder or
  reprice the whole book in a single instruction.

### Page 6 — The punchline · ~8s

- **On-slide:** Right now, my 2021 laptop is the most liquid place to
  trade these currencies onchain.
- **Visual:** The presenter's actual laptop, or the multi-market TUI
  showing several books quoting at once.
- **Spoken:** "And just like that, during this demo, my 2021 MacBook is
  the most liquid venue on Solana for these pairs."

### Page 7 — Why this will fail · ~12s

- **On-slide:** Why won't this work? Arc, Tempo, Hibachi, Canton.
- **Visual:** The competitor logos as a wall closing in.
- **Spoken:** "The honest risk: everyone wants onchain settlement. Arc,
  Tempo, and Hibachi are building payment-and-settlement rails, and
  Canton is doing regulated onchain markets. Any of them could decide
  FX is theirs."
- **Note:** This slide is deliberate — an investor respects that we
  named the threat first (Clay's + Alex's advice). The rebuttal is the
  very next page. Full framing in Appendix → "Why this will fail."

### Page 8 — Why it will work · ~8s

- **On-slide:** They're private, permissioned, or walled — we're the
  open venue.
- **Visual:** A single open door vs. a row of locked ones.
- **Spoken:** "But those are private, heavily permissioned, or viewed
  as adversarial — closed gardens. Dropset is the open, neutral venue:
  anyone can quote, anyone can trade, and we're beating them to the
  open market."

### Page 9 — How we grow · ~6s

- **On-slide:** We bootstrap the liquidity ourselves — like
  Hyperliquid.
- **Visual:** A curve of depth growing; logos of Colosseum partners
  (Altitude, Cargobill).
- **Spoken:** "We seed the markets ourselves the way Hyperliquid did,
  and Colosseum partners like Altitude and Cargobill already need to
  source FX onchain — that's our first real demand."

### Page 10 — Team & close · ~4s

- **On-slide:** Built by people who've built exchanges.
- **Visual:** Alex + Judy, then the Dropset wordmark.
- **Spoken:** "I've built two exchanges and an onchain order book
  before, and cofounded Econia Labs. Judy runs operations — she's
  getting us banked across the stablecoin providers, onramps, and
  accounting. Dropset — onchain Forex, on Solana."

---

## Appendix (not on slides — for the Q&A / the call)

Mert: keep the nuance off the deck; put it here and cover it if you get
a call. This is the material to have ready when an investor grills.

### Team, full

- **Alex — product / exchange design.** Exchange designer; has built
  two exchanges and one CLOB before; previously cofounded Econia Labs
  (authored Econia, the onchain order book on Aptos, ~$500M cleared).
  Founder stays on product.
- **Judy — operations.** Formerly EA at Dragonfly. Running the
  operational spine: opening accounts with the stablecoin providers and
  onramps, plus accounting — the work that gets an FX venue actually
  banked.

### Architecture — the moat (custody / quoting separation + parallelism)

Verified against the on-chain program:

- **One cold `leader` key** (in 1Password) custodies all vaults'
  inventory.
- **One hot `quote_authority` key per market**, delegated on-chain via
  `set_quote_authority`. A hot key can *only* set its market's
  price/profile — it **cannot touch inventory**. A leaked hot key's
  blast radius is one market's quotes.
- Each market is a **separate account** signed by its **own** key, so
  Solana's Sealevel runtime **quotes every market in parallel** — no
  shared writable account to serialize on.
- **Why not batch all quotes in one transaction?** Gas is
  per-signature (~$0.001), not per-instruction, so batching saves
  nothing — and one transaction would serialize the markets and not
  even fit the 1,232-byte limit. Independent transactions = parallel +
  fault-isolated.
- Talking point: "One cold key custodies the vaults; hot delegates
  quote — fully parallel, each blast-radius-isolated. Right now, on
  this laptop, I'm repricing several FX order books in parallel on
  Solana."

### Why this will fail — the steelman, and the answer

- **"Arc / Stripe / Tempo are good enough; open access doesn't
  matter."** — Answer: they're private or heavily permissioned rails.
  The moment FX needs a *neutral* venue where anyone can make a market
  and anyone can trade, a closed garden can't serve it. We're building
  the open venue and getting there first.
- **"Hibachi / Canton are direct competitors."** — Hibachi and the
  Arc/Tempo camp are chasing settlement; Canton is regulated onchain
  markets. But they're permissioned or walled — different animal from
  an open, composable FX book.
- **"Why wouldn't Jupiter or a big DEX just do this?"** — They aren't
  focused on FX, and we're beating them to it. It's an innovator's
  dilemma: an open, FX-specialized venue only makes sense for a small,
  focused team to chase right now — the volume (a few million a day
  today) is too small to move a giant, and we'll be here for the next
  10x as payments come onchain.
- **"Show me you've thought about every angle."** — The point of this
  section: an investor wants to see the failure modes named and
  answered, not hidden.

### Lazy-VC questions to pre-empt

Have crisp one-liners ready for the questions a VC asks without reading
the deck:

- "What's the market?" → FX, the biggest market on earth, with no
  liquid onchain home yet.
- "Who's using it?" → Live on mainnet now; Colosseum partners (Altitude,
  Cargobill) are the first FX demand.
- "Why you?" → We've built exchanges before; this is our domain.
- "Why now?" → Currencies are only just arriving onchain (~14 today),
  and payments are following.

### Demo failure modes (rehearse the fallbacks)

Cascade, most-live to least:

1. **Mainnet swap** (Page 4) — primary.
1. **Localnet flash-liquidity** (Page 5) — primary proof of
   manufacturing liquidity; also the fallback if mainnet is
   unreachable.
1. **Recorded video** — last resort.

Pre-run checklist: clear state, `make demo`, confirm inventory hasn't
slipped from the day before, confirm the price feed has a live key (not
bogus / rate-limited), confirm the build compiles.

### Open questions for reviewers

- Is "$9T/day" (Page 2) the framing we want, given Mert's no-TAM rule?
  Current call: keep it as the *gap*, said aloud, not as a slide stat.
- Do we name specific pairs on Page 6 (euro, franc, peso) or keep it
  generic?
- Page 9: are Altitude and Cargobill OK to name publicly on a slide, or
  keep them verbal-only?
