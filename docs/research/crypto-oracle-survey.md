<!-- cspell:word Chainlink -->

<!-- cspell:word Switchboard -->

<!-- cspell:word Redstone -->

<!-- cspell:word Stork -->

<!-- cspell:word Supra -->

<!-- cspell:word Crossbar -->

<!-- cspell:word Bandchain -->

<!-- cspell:word Schnorr -->

<!-- cspell:word Bitstamp -->

<!-- cspell:word Chronicle -->

<!-- cspell:word Multicall -->

<!-- cspell:word merkle -->

<!-- cspell:word Bitget -->

<!-- cspell:word FTSO -->

# Crypto-native oracles as Pyth-tier alternatives

Research survey, 2026-09-03. Wires nothing; the deliverable is the
ranked comparison below plus the follow-ups named at the end.

Context: the Pyth Hermes key gate closed our only wired oracle's free
read path, which opened the question of what replaces or backs up the
oracle tier. This is the third of three surveys — the siblings cover
issuer rates with stablecoin venues, and traditional FX venues.

## What the survey is judged against

**The roster.** Fifteen fiats: AUD, BRL, CAD, CHF, EUR, GBP, IDR, JPY,
MXN, MYR, NGN, SGD, TRY, USD, ZAR. USD is the quote leg throughout, so
coverage is scored out of the **14 non-USD** currencies. EUR/USD is the
flagship; AUD/USD matters because an AUD stablecoin issuer is a
prospective first customer. The thin currencies — SGD, MXN, NGN, IDR,
MYR, TRY, ZAR — are where oracles fail, so each was checked
individually rather than credited to a "major FX" claim.

**On-chain residency is not a ranking advantage.** The consumer is the
maker bot reading **off-chain** to set a reference price it uploads, so
what counts is the cheapest reliable off-chain read. An EVM-only oracle
with a good free read path ranks fine; a Solana-resident one is one more
read path, not a category win. A second consumer, the standing analytics
warehouse, is scored separately — a source that quotes well live but
cannot be collected on a standing basis only half-qualifies.

**Uncertainty output is one scored column, not the organizing
question.** Our fusion estimator
(`fair-value/src/fusion.rs`, `Fusion::measurement_variance`) prefers a
published confidence as the measurement half-width, but where none is
published it substitutes a per-source-class noise fraction and inflates
by the square root of the reading's age. A candidate is skipped only
when its value or variance is non-finite or non-positive — deliberately
never defaulted, because a made-up variance would be the same
fabricated-parity failure the basis path refuses.

So a no-confidence oracle is **not disqualified**; the cost is that we
assign the variance rather than receive it. What the survey therefore
records per candidate is the material needed to *calibrate* such a
fraction: the real deviation threshold, the publisher count and
identity, and any dispersion derivable by the reader.

**The venue principle applies unchanged.** A source earns a feed when it
moves the fair-value estimate; it is a pricing input, never a benchmark.
The standing precedent is instructive: a high-volume venue was scrapped
for **lagging**, not for thinness, with an explicit warning that citing
thinness invites a re-litigation the volume figure would win. An oracle
that re-serves aggregators we already wire is that same case under a new
name, which makes independence a ranking axis rather than a footnote.

**Free-first.** A free tier whose terms are unpublished classifies as
**credentialed**, not free.

## Roster coverage

Verified against live feed catalogues and, where possible, live reads —
not against marketing pages.

| Ccy | Chainlink | Stork | Band | Redstone | Supra |
| --- | --------- | ----- | ---- | -------- | ----- |
| AUD | yes       | yes   | yes  | no       | yes   |
| BRL | yes       | no    | yes  | no       | yes   |
| CAD | yes       | yes   | yes  | no       | yes   |
| CHF | yes       | yes   | yes  | no       | yes   |
| EUR | yes       | yes   | yes  | yes      | yes   |
| GBP | yes       | yes   | yes  | yes      | yes   |
| IDR | yes       | no    | no   | yes      | yes   |
| JPY | yes       | yes   | yes  | yes      | yes   |
| MXN | yes       | no    | no   | no       | no    |
| MYR | no        | no    | yes  | yes      | no    |
| NGN | yes       | yes   | no   | no       | yes   |
| SGD | yes       | yes   | yes  | yes      | yes   |
| TRY | yes       | yes   | yes  | yes      | yes   |
| ZAR | yes       | yes   | no   | no       | no    |

Totals out of 14: **Chainlink 13, Supra 11, Stork 10, Band 10,
Redstone 7.**

The cells are not all the same kind of evidence. Band's are
live-confirmed as actively priced; **Stork's are registry presence
only** — its public dashboard shows no FX, and confirming liveness needs
a token — so read that column as catalogue coverage, not proven
liveness.

Not in the table: **Switchboard 0** (the maintained Surge catalogue is
18,743 symbols with zero FX) and **DIA 2** (only EUR and GBP resolve;
the documented others 404), both failing on coverage alone. **Chronicle**
is excluded on different grounds — **read-gating**, not coverage: its
feeds cannot be read at all without whitelisting, and its coverage
finding is explicitly an absence of evidence rather than proof of
absence, so it is not a coverage failure this survey can assert.

**MXN, MYR and ZAR are the scarce currencies.** MXN exists only on
Chainlink; MYR only on Band and Redstone; ZAR only on Chainlink and
Stork. Every other roster currency is carried by at least three
candidates.

## Ranked comparison

### 1. Chainlink — strongest overall

Best coverage (13/14), a genuinely free keyless read, and the cleanest
redistribution position of any candidate here.

The read path is `eth_call latestRoundData()` against any EVM RPC —
no signup, roughly 200ms, and Multicall batches the whole roster into
one call. Because feed values are **public chain state rather than a
vendor-served endpoint**, the terms binding our warehouse and Grafana
panels are the RPC provider's, not Chainlink's. That sidesteps the
redistribution risk gating Stork and Redstone — a property it shares
with Band, and with nothing else surveyed.

**Liveness is chain-dependent, and this is the finding that matters.** A
probe of 13 feeds during FX open hours returned data on all of them, at
ages that split the product in two: Polygon EUR/USD 0 minutes and Base
EUR/USD 31, against Ethereum EUR/USD **502** and SGD **717**. Cross-chain
EUR/USD agreed to about 0.04%. Ethereum's FX feeds are live but
hours-stale by design and are **not quotable**. Polygon and Base were
measured fresh; other deployments were not probed and need their
configured cadence checked rather than assumed. Treating "Chainlink has
EUR/USD" as a single fact would be wrong.

**The 13/14 is counted roster-wide across networks, and the quotable
subset is NOT established.** Coverage was enumerated over eight
networks, while the recommendation below quotes only from feeds whose
configured cadence is fast enough — and nothing here shows that all 13
currencies have such a deployment. The one thin-currency reading taken
points the wrong way: SGD was probed on Ethereum, at 717 minutes. So
effective quotable coverage may be materially below 13, and pinning it
down per currency is the first task of any wiring work.

Cadence is deviation-plus-heartbeat configured per feed and per chain —
Ethereum FX at 0.15-0.5% / 24h, Polygon FX at 0.01-0.1% / 27-60s. Forex
feeds observe 18:00 ET Sunday to 17:00 ET Friday plus a holiday
schedule, which matches our own session calendar.

No uncertainty is published. The configured **deviation band is itself
an implicit error bar** — a 0.1% threshold bounds how far the rate can
move before an update is forced — and it is published per feed, which
makes it the most defensible basis for a class noise fraction found here
short of a per-publisher array.

Provenance is unproven rather than disproven: per-feed sources are
undisclosed, so overlap with vendors we already wire cannot be ruled
out. What is independent is the aggregation and operator set — a median
across a node committee fails differently from any single vendor we
poll.

The warehouse path is the best of any candidate: poll `latestRoundData()`
on a cadence, or walk `getRoundData()` and `AnswerUpdated` logs for
backfill to full chain history. A collector must handle round ids being
non-contiguous across aggregator upgrades.

Avoid two Chainlink products: **Data Streams** is explicitly paid with no
free tier and fails free-first, and the **24/7 forex feeds** derive a
rate through stablecoin arbitrage and self-describe as relying on a
single provider that can "persistently and materially diverge" from
interbank rates.

### 2. Band — the complement that closes the roster

Band's `feeds` module carries 22 fiat FX signals as first-class live
feeds, ten of them ours, read keyless over a public Cosmos LCD REST
endpoint with batched multi-signal requests. Values were confirmed
actively priced rather than merely registered.

**Band carries MYR, the one currency Chainlink lacks** — and unlike
Redstone, which also has MYR, Band is permissionless public chain state.
The same data is readable from any third-party endpoint or a self-hosted
node, so a self-hosted or multi-endpoint reader carries no vendor
redistribution restriction. Leaning on Band's own public node does, since
that endpoint publishes no terms or rate limit.

Cadence is the top tier at 60s heartbeat / 50bps deviation. Note that
50bps on EUR/USD is roughly 58 pips, so in practice FX updates are
**heartbeat-driven at ~60s, not deviation-driven** — a cross-check
rather than a sub-minute reaction source.

No uncertainty output. Aggregation is a weighted median of three
Band-operated sources with a minimum cumulative weight of 1, meaning a
**single surviving source satisfies the feed** — dispersion is neither
computed nor exposed, and the three-source floor is shallow.

Provenance is the soft spot: all FX signals route through three
Band-operated proxies whose upstream vendors are not disclosed in the
open-source repository. Independence from OANDA, TwelveData and
AlphaVantage is therefore unverifiable rather than established.

There is no historical endpoint, so backfill is impossible; standing
collection means polling and storing ourselves, which is what the
warehouse does anyway, and public chain state carries no storage bar.

Adding a missing symbol is not cheap for us: new signals enter through
the Signaling Hub by staking for voting power.

### 3. Stork — best calibration material, behind a sales wall

Ten roster currencies including both flagships and, unusually, three
thin ones plus NGN. Cadence is a published and specific 500ms-or-0.1%
spec observable off-chain, and FX assets carry market-status semantics
(Closed / Regular / Extended / After Hours) that line up with our 24/5
session calendar.

Its distinguishing fact: `/v1/prices/latest` returns a **`signed_prices`
array of individual publisher submissions** alongside the aggregate,
with a publisher merkle root. Cross-publisher dispersion is therefore
computable client-side and publisher count is the array length — genuine
dispersion rather than a median-of-N restatement, and the best
calibration input in this survey.

Two facts point opposite ways and must be reported together rather than
netted: that per-publisher array is the strongest calibration material
found, while the publisher set is **unnamed and uncounted** — described
only as HFT firms, exchanges and prediction markets — which is exactly
what makes calibration untrustworthy for FX specifically.

Access is `Authorization: Basic` with no published pricing and no
self-serve signup; acquisition is a sales email. It **classifies
credentialed**. The terms of service could not be fetched; indexed text
shows a Restrictions section and a "limited, non-exclusive,
non-transferable, and revocable license". Redistribution and storage
language is unread, which is the single unresolved commercial risk and
the same shape as our existing rate-API precedent.

The warehouse path would be strong — a real OHLC history endpoint at
resolutions from one minute upward, comfortable at 5 requests/second —
but depth is unstated and storage rights are unknown.

Registry presence is not liveness: the public dashboard shows no FX at
all, and confirming FX liveness needs a token.

### 4. Redstone — free read, adverse terms

A genuinely free keyless gateway returning signed FX values with no
wallet, transaction or key; a reader takes one field and may optionally
verify signatures. Sub-second, trivial effort.

**Cadence.** The pull model has no deviation threshold or heartbeat at
all: gateway packages are re-signed continuously (sub-minute) and the
reader triggers the read, so freshness is whatever the signers last
published. The on-chain push feeds do carry deviation-plus-heartbeat,
but none of those are fiat FX.

**Uncertainty (fact).** None published. The EUR response carried five
independent signed packages with no confidence, interval or standard
deviation field — so dispersion across the five signers is computable
by the reader, the same kind of calibration material Stork offers, from
fewer publishers.

It fails on three counts. Coverage is 7/14 and **AUD is absent**, which
is the first-customer pair. Provenance is measurably poor: the legacy
source map for EUR was a basket of crypto venues — Kraken, Binance,
MEXC, Bitstamp, Bitget, Coinbase, OKX — which is stablecoin pricing
rather than interbank FX and re-serves Kraken and Coinbase, both already
in our stack. Whether the modern feeds share that basket is
unverifiable, since the gateway publishes no source map.

Decisively, the published Terms of Use prohibit automated access,
commercial use absent approval, and systematically retrieving content to
compile a database. That is squarely adverse to a standing warehouse and
to re-exposing values in our indexer API or Grafana. There is also no
history endpoint, so backfill depth is zero.

The legacy REST API is **dead** — roughly four weeks stale, with only
EUR and GBP live. Do not build on it.

### 5. Switchboard — no FX to give

Disqualified on coverage alone: the maintained Surge catalogue is 18,743
symbols with **zero FX**, and all 14 non-USD roster currencies are
absent. Feeds are permissionless, so EUR/USD is definable but not
extant — and a feed we define is a hosted job runner over vendors we
already wire, which is a name rather than a source.

One correction worth recording, because the opposite is widely assumed:
Switchboard's pull model imposes **no transaction, crank, or funded feed
account** on an off-chain reader. Crossbar `/simulate` and
`/gateways/fetch_signatures` both return a price with no transaction,
and canonical accounts are derived deterministically and created
automatically. The two-instruction cost applies only to landing a price
on-chain. Its read mechanics are clean; only its catalogue is empty.

Two caveats had it carried FX: `/simulate` returns **unsigned local job
execution** — the docs warn simulation can succeed where signed updates
fail oracle-side validation — and the public instance is best-effort,
rate-limited by IP, which lands on our shared-egress budget.

### 6. DIA — re-serves a source we already wire

Only EUR and GBP resolve on the keyless path; the documented NGN, BRL,
JPY and CHF pairs returned 404 on every endpoint reachable. That alone
disqualifies it as an FX tier.

The provenance finding is the more useful one: DIA's EUR is a
volume-weighted quote over **Kraken, Binance and Bitstamp**. That is a
crypto-venue EUR, not interbank FX; Kraken we already wire directly, and
Binance is geo-blocked for us. So DIA adds an opaque moving-average
filter on top of data we can read at the origin.

The free tier is self-described as "built for testing and exploration"
with unpublished rate limits and unreachable terms, so it classifies
credentialed and is an affirmative signal against standing collection.

### 7. Chronicle — read-gated

Every read entry point on the feed contract carries a whitelist
modifier, so a non-whitelisted call cannot read a feed. Self-whitelisting
is testnet-only; mainnet access is a support-ticket business
relationship. No roster currency could be confirmed; the one
euro-adjacent feed is a EURC stablecoin feed rather than interbank
EUR/USD. Aggregation collapses on-chain to a single value plus age via a
Schnorr-aggregated signature, so no dispersion is exposed.

Two undocumented side doors exist — update events are readable via logs,
and the price struct is ordinary storage readable directly, since access
control is a Solidity construct rather than an RPC one. Both are
unsupported and layout-fragile. They mean "unreadable" is technically
false; they are not a basis to build on.

Its documentation and dashboard returned 429 or 403 to every automated
fetch across the survey, which is disqualifying for a standing collector
independent of any terms.

## Peers triaged, not surveyed

**Supra** is the highest-value follow-up: 11/14 as live 24x5 forex
feeds, the broadest coverage of any candidate not already recommended.
Note what it does **not** buy — it carries none of MXN, MYR or ZAR, so it
relieves none of the scarcity above. Its value is a second independent
source for currencies that already have several, not a fix for the
single-point-of-failure ones. Its REST and history APIs are Early Access
behind a form, so it classifies credentialed and was not surveyed to
depth. On-chain push feeds are
readable via public RPC on many chains; that effort is unverified, as is
whether any uncertainty field exists.

**API3 is retiring** its equities, forex and commodities feeds. Do not
pursue.

**Flare FTSO v2** advertises many feeds but no fiat FX pair list could be
verified. Cheap to check; the coverage claim is unsubstantiated.

**eOracle**, **Ojo**, **Umbrella** and Chaos Labs' **Edge** showed no FX
coverage. Edge is Solana-native and focused on perpetual futures, and
markets itself on outlier detection rather than a published interval.

## Recommendation

**Chainlink as the primary oracle input, with Band closing the gap.**
Their union covers all 14 non-USD roster currencies on paper, and both
are public chain state rather than a vendor-served endpoint — so
neither carries the redistribution exposure that gates Stork and
Redstone, on the live read path or in the warehouse.

Four conditions on that recommendation:

1. **Select each Chainlink feed by its configured cadence, not by
   chain.** Chainlink is a price network read directly over RPC; a
   chain is where a given feed's deviation and heartbeat parameters
   are configured, not the oracle's identity. Take the deployment whose
   configured band meets the quoting requirement — Polygon's EUR/USD at
   0.01% / 27s is the measured example — and never quote Ethereum's FX
   feeds, which run a 24h heartbeat and measured 502 and 717 minutes
   stale. Any deployment not probed here needs its cadence read from the
   feed directory rather than assumed.
1. **Read Band from a third-party or self-hosted endpoint, never its
   own public node.** This condition is load-bearing, not hygiene: that
   node publishes no terms and no rate limit, which is precisely the
   shape this survey's own free-first rule classifies as
   **credentialed** — the same rule applied against DIA. Band is free
   and clean *as public chain state*, read from an endpoint we control
   or from several; it is not free by virtue of Band operating a public
   LCD.
1. **Treat Band as a ~60s cross-check, not a quoting source** — on
   aggregation integrity, not cadence. Its 60s heartbeat is in fact a
   *tighter* staleness bound than most Chainlink deployments, and its
   50bps deviation trigger is inert (the body shows it never fires for
   FX). The reasons to demote it are that its aggregate carries a
   minimum cumulative weight of 1, so it can silently collapse to a
   single unnamed publisher, and that its three proxies' upstream
   vendors are undisclosed — which makes it a cross-check whose
   independence from the sources being checked is unverifiable.
1. **Neither publishes uncertainty**, so both enter the fusion at an
   assigned class noise fraction. Chainlink's per-feed deviation band is
   the most defensible basis for calibrating it — but that band is
   published per feed, so it must be read per deployment rather than
   assumed from the two configs quoted here.

**Redundancy within the recommended pair is worse than the roster-wide
scarcity above suggests.** Five currencies are carried by only one of
the two: **IDR, MXN, NGN and ZAR** on Chainlink alone, and **MYR** on
Band alone. Roster-wide scarcity is the wrong statistic once a pair has
been chosen; this is the operative one.

Qualifying Supra would second **IDR and NGN** — two of those five — and
nothing else, since it carries none of MXN, MYR or ZAR. **MXN, MYR and
ZAR have no identified path to a second usable carrier at all.** MYR is
the sharpest case: its only other carrier is Redstone, which this
survey rules out on terms, so it rests on one source with no candidate
behind it.

## Not verified

Stated plainly rather than smoothed over, because several bear directly
on the recommendation:

- **Chainlink**: MYR absence covers eight networks, not all; per-feed
  data providers are undisclosed, so independence is unproven either
  way; the liveness probe is a single point in time, not a monitored
  series; the terms page would not render.
- **Chainlink**: the rate limits and quota of a public EVM RPC are
  unquantified here, and that is the read path we would poll per feed
  on a standing cadence against a shared egress IP. Multicall batching
  and provider fungibility mitigate it; neither measures it.
- **Band**: upstream FX vendors are opaque by construction.
- **Stork**: FX feed liveness, history backfill depth, redistribution
  and storage terms, publisher identities, and whether any free tier or
  a token issuable to a party our size exists.
- **Redstone**: whether the modern FX feeds share the legacy
  crypto-venue basket; gateway rate limits are unpublished.
- **Chronicle**: the full feed list could not be enumerated — the
  "no fiat FX" finding is absence of evidence from reachable sources,
  not proof of absence.
- **Supra**: key policy for forex, and whether its feeds carry any
  uncertainty field.
- **DIA**: terms of service, and its claimed non-EUR/GBP pairs.

No feed addresses, thresholds or prices were inferred where they could
not be read.
