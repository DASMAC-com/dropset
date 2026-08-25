// cspell:word followable
// cspell:word FUSD
// cspell:word userinfo

//! The HTTP-REST poll transport (`http` feature).

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::time::{sleep_until, Instant};

/// The floor between two requests on one client, applied unless a source
/// raises it with [`HttpClient::with_min_interval`]. Collectors and the maker
/// share one host and one egress IP and keyless tiers limit by IP
/// (docs/data-feeds.md §10), so a floor has to exist somewhere below every
/// adapter; this is that backstop.
///
/// It is a floor, not a cadence: steady-state polling rate belongs to the
/// runner's `RunConfig::poll_interval`, and this only binds on back-to-back
/// requests such as a paged backfill.
///
/// **It is a backstop, not a budget — do not assume it fits your venue.** 250 ms
/// is ~240 requests a minute, which most venues in this crate do not allow, and
/// the runner tight-loops while a source backfills, so this is exactly what
/// paces a cold catch-up. A new adapter should look its venue's limit up and
/// state its own floor; an adapter that keeps this default should say why, at
/// the point where it declines to raise it. Which venues do which is recorded
/// once, in [`crate::venues`] and docs/data-feeds.md §10 — deliberately not
/// enumerated here, since a transport that names its consumers goes stale the
/// first time one of them changes.
pub(crate) const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(250);

/// The response-body ceiling, applied unless a source raises it with
/// [`HttpClient::with_max_response_bytes`]. Every venue this crate polls
/// answers in kilobytes; the cap exists so a wedged or hostile endpoint cannot
/// make the consumer allocate without bound, pairing a size bound with the
/// time bound the request timeout already provides.
const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// How long a 429 holds the client off when the venue sends no usable
/// `Retry-After`.
const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);

/// The longest cooldown a `Retry-After` can impose. A venue asking for hours
/// would otherwise wedge the feed silently; past this the source errors on its
/// own cadence and the operator sees it.
const MAX_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(300);

/// The redirect statuses `HttpClient::new`'s policy declines to follow — the
/// ones `reqwest` would have followed had the policy allowed it. Deliberately
/// not the whole 3xx class: 300 (Multiple Choices), 304 (Not Modified) and 305
/// (Use Proxy) are not followable redirects, and reporting one as a refused
/// redirect would be a false diagnosis.
const REFUSED_REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];

/// What a credential query parameter's value is rewritten to in an error URL.
/// A placeholder rather than a removal, so the error still shows that the
/// request *was* authenticated — a missing key and a rejected one are different
/// diagnoses.
const REDACTED: &str = "REDACTED";

/// The query-parameter names whose values are safe to render in a transport
/// error. Everything else is redacted.
///
/// This is an **allow-list**, and the inversion is the point. The redaction it
/// replaced was a deny-list keyed on names registered through
/// [`HttpClient::with_secret_query_param`], which covered only the credentials
/// a caller remembered to register: an adapter that hand-passed a key through
/// [`HttpClient::get_json`]'s `query` under an unregistered name was covered by
/// nothing at all, and its error text reaches the feed-health last-error column
/// that the read-only dashboard role can read. Default-deny makes that class of
/// mistake safe by construction rather than by discipline.
///
/// **Scoped to the query, and deliberately not claimed beyond it.** The
/// redaction rewrites query pairs; a credential carried in a **path segment**
/// or in URL **userinfo** is rendered by the same error and is not reached
/// here. That is not a regression — the deny-list did not cover them either —
/// but it bounds what "safe by construction" buys, and the boundary is live
/// rather than theoretical: this venue set already includes a provider whose
/// keyed tier authenticates by path (`/v6/<key>/latest/...`) where its keyless
/// tier does not. A keyed adapter must authenticate by header or by query, not
/// by path.
///
/// The entries are the benign parameters the wired adapters actually send —
/// what a failed paged backfill is diagnosed from: which symbol, which
/// interval, which window. A new venue with a new benign name adds it here;
/// until then that value renders as [`REDACTED`], which costs one round of
/// diagnosis. That is the correct direction to fail in, and it is why this list
/// is not derived from the query at run time: a name only earns its place by a
/// human deciding it carries no credential.
const BENIGN_QUERY_PARAMS: &[&str] = &[
    "base",
    "end",
    "end_date",
    "from",
    "from_symbol",
    "function",
    "granularity",
    "ids",
    "ids[]",
    "interval",
    "outputsize",
    "pair",
    "parsed",
    "price",
    "start",
    "start_date",
    "symbol",
    "symbols",
    "timezone",
    "to",
    "to_symbol",
    "vs_currencies",
];

/// One credential query parameter: a name, and a value behind a `Debug` that
/// never prints it.
///
/// A query parameter's value is not a `HeaderValue`, so
/// `HeaderValue::set_sensitive` cannot reach it — and the header path's own
/// test pins the invariant that a credential stays redacted under *any later*
/// `#[derive(Debug)]` on the path. A plain `(String, String)` would break that
/// symmetry: OANDA's header key would still render as `Sensitive` while this
/// one rendered in clear. Making the redaction a
/// property of the **type** is what keeps the guarantee from depending on
/// which types happen not to derive `Debug` yet — the same reasoning
/// [`HttpClient::with_secret_header`] is built on.
#[derive(Clone)]
struct SecretParam {
    name: String,
    value: String,
}

impl fmt::Debug for SecretParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The name renders in clear — both venues publish it in their public API
        // docs, so it is not the secret, and seeing it is what makes a redacted
        // render diagnosable. The value never renders.
        write!(f, "{}={REDACTED}", self.name)
    }
}

/// A small JSON-over-HTTPS client REST poll sources compose: a base URL, a
/// shared `reqwest` client, and [`HttpClient::get_json`]. The Coinbase
/// reference feed uses it first; the FX / Circle-rate feeds follow
/// (docs/data-feeds.md §4). It is a transport, not a `Source`: a feed wraps it
/// in its own [`crate::Source`] that decodes the JSON into typed records and
/// computes its cursor.
///
/// It is also where per-venue rate-limit discipline lives (docs/data-feeds.md
/// §10): requests are paced by a minimum interval, a 429 records a cooldown
/// the next request waits out, and a response body is capped.
#[derive(Clone)]
pub struct HttpClient {
    base_url: String,
    client: reqwest::Client,
    /// Headers sent on every request — the seam for an auth key a source's API
    /// requires on each call (OANDA's `Authorization: Bearer`). A credential is
    /// set with [`HttpClient::with_secret_header`], anything benign with
    /// [`HttpClient::with_header`].
    headers: HeaderMap,
    /// Credential query parameters appended to every request — the seam for a
    /// venue that authenticates by URL rather than by header (Alpha Vantage's
    /// and Twelve Data's `apikey`). Set with
    /// [`HttpClient::with_secret_query_param`].
    ///
    /// Registering a credential here no longer drives the redaction:
    /// [`HttpClient::redact_query`] is default-deny against
    /// [`BENIGN_QUERY_PARAMS`], so an unregistered name is redacted just the
    /// same. What registration still buys is the value never rendering through
    /// [`SecretParam`]'s `Debug`, and the transport appending the key itself so
    /// no call site has to carry it.
    secret_query: Vec<SecretParam>,
    min_interval: Duration,
    max_response_bytes: usize,
    /// The earliest instant the next request may go out, shared across clones
    /// so a cloned client draws on the same venue budget rather than opening a
    /// second one. `None` until the first request reserves a slot.
    next_allowed: Arc<Mutex<Option<Instant>>>,
}

impl HttpClient {
    /// A client rooted at `base_url` (e.g. `https://api.exchange.coinbase.com`),
    /// with a request timeout, a stable user agent, and the default pacing and
    /// body-size bounds.
    ///
    /// **Redirects are refused, which is a credential boundary, not a
    /// preference.** `reqwest`'s default policy follows up to 10 redirects and
    /// strips credentials across a cross-host hop *by header name* only —
    /// `Authorization`, `Cookie`, `cookie2`, `Proxy-Authorization`,
    /// `WWW-Authenticate` — and never consults the sensitive marking
    /// [`HttpClient::with_secret_header`] applies. A custom-named key header is
    /// not on that list, so it would be replayed verbatim to whatever
    /// third-party host a redirect named: wire-to-a-third-party, the one sink a
    /// sensitive flag cannot cover.
    ///
    /// **This is preventive, not the closing of a live hole**, and the
    /// distinction is worth stating precisely because the roster is what
    /// changed. Every keyed adapter today is safe *by accident of naming*:
    /// OANDA's bearer rides `Authorization`, which is on the strip list, and
    /// the two query-parameter venues touch no header at all. The header that
    /// motivated this boundary — a custom-named venue key — was retired when
    /// CoinMarketCap moved to its keyless route. So the exposure is currently
    /// unrealized, and it re-opens silently the first time a venue
    /// authenticates by a custom header, which is exactly how the retired one
    /// worked. Pinning the policy is what makes the guarantee a property of the
    /// transport rather than of the current roster.
    ///
    /// A feed poller has no legitimate cross-host redirect: every venue this
    /// crate polls is a canonical JSON API host answering directly (verified by
    /// probe — all eight answer without a 3xx). Refusing outright also fails
    /// *loudly* if a venue ever starts redirecting, which is the better of the
    /// two failure modes — the alternative is a silent key disclosure. Widen
    /// this to a host-scoped policy only with a venue that demonstrably needs
    /// one.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("dropset-feeds/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("build HTTP client")?;
        Ok(Self {
            base_url: base_url.into(),
            client,
            headers: HeaderMap::new(),
            secret_query: Vec::new(),
            min_interval: DEFAULT_MIN_INTERVAL,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            next_allowed: Arc::new(Mutex::new(None)),
        })
    }

    /// Add a header sent on every request (an errored `name` / `value` is
    /// rejected here rather than per request). Chains from [`HttpClient::new`].
    ///
    /// This is for a **benign** header — a response-format preference, an
    /// `Accept`. A credential goes through
    /// [`HttpClient::with_secret_header`] instead.
    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self> {
        let (name, value) = Self::header_pair(name, value)?;
        self.headers.insert(name, value);
        Ok(self)
    }

    /// Add a **credential** header sent on every request — an API key, a bearer
    /// token — with its value marked sensitive.
    ///
    /// Same wiring as [`HttpClient::with_header`] plus the one difference that
    /// is the whole point: `HeaderValue::set_sensitive` keeps the value out of
    /// a `Debug` render of the header map, and tells the HTTP/2 stack not to
    /// store it in HPACK's dynamic table, where a compression side channel
    /// could otherwise recover it.
    ///
    /// Nothing on the path to a header map derives `Debug` today, so the
    /// exposure this closes is latent rather than live — which is precisely why
    /// it belongs on the constructor. Whether a key leaks must not be decided
    /// by which types happen not to derive `Debug` yet.
    ///
    /// Every adapter that authenticates **by header** goes through here
    /// (`OandaCandles` is the only one today), as must any added later. A venue
    /// that authenticates by **query parameter** instead — Alpha Vantage and
    /// Twelve Data both do, and so touch no header at all — goes through
    /// [`HttpClient::with_secret_query_param`], which closes the different sink
    /// a URL-borne credential has.
    ///
    /// **A cross-origin redirect is the other sink this constructor cannot
    /// reach, and it discriminates by header *name*.** On a cross-host hop
    /// `reqwest` strips only the well-known credential headers —
    /// `Authorization`, `Cookie`, `cookie2`, `Proxy-Authorization`,
    /// `WWW-Authenticate` (0.12.28, `redirect::remove_sensitive_headers`) — and
    /// never consults `set_sensitive`, which governs `Debug` rendering and
    /// HPACK indexing only. A **custom-named** key header is not on that list.
    /// No adapter has one today (OANDA's `Authorization` is stripped), so
    /// nothing is exposed by name — but that is a property of the current
    /// roster, not of this constructor. [`HttpClient::new`] therefore pins
    /// `redirect::Policy::none()`, which is what makes the guarantee hold for a
    /// venue not yet written.
    pub fn with_secret_header(mut self, name: &str, value: &str) -> Result<Self> {
        let (name, mut value) = Self::header_pair(name, value)?;
        value.set_sensitive(true);
        self.headers.insert(name, value);
        Ok(self)
    }

    /// Validate a header `name` / `value` pair for the two constructors above.
    ///
    /// The error context names only the header *name*, never the value: a
    /// credential that fails to encode must not echo itself into whatever log
    /// the caller's error lands in.
    fn header_pair(name: &str, value: &str) -> Result<(HeaderName, HeaderValue)> {
        let header = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name {name:?}"))?;
        let value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid header value for {name:?}"))?;
        Ok((header, value))
    }

    /// Add a **credential** query parameter appended to every request — the
    /// counterpart to [`HttpClient::with_secret_header`] for a venue that
    /// authenticates by URL (Alpha Vantage's and Twelve Data's `apikey`).
    ///
    /// A URL-borne key cannot be protected the way a header-borne one is: no
    /// header marking reaches it, and the sink is different. A
    /// `reqwest::Error` carries the **effective** URL — query string included —
    /// and renders it in its own `Display`, so the key surfaces in any `{:?}`
    /// of the resulting `anyhow` chain, which is exactly what a top-level error
    /// handler logs. It needs no hostile venue, only an ordinary request
    /// failure.
    ///
    /// Carrying the key *here* rather than in the caller's per-request query is
    /// what closes it: the transport appends the credential itself, and
    /// [`SecretParam`]'s `Debug` keeps the value out of any render of the
    /// client.
    ///
    /// An adapter that hand-passes a key through `get_json`'s `query` under a
    /// name registered nowhere here **no longer bypasses the redaction** —
    /// [`HttpClient::redact_query`] is default-deny, so an unregistered name is
    /// redacted like any other non-benign parameter. That is a backstop, not a
    /// license: register the credential here anyway, because only registration
    /// keeps it out of a `Debug` render of the client and off every call site.
    ///
    /// Registration is by **name**, so a caller that also passes a parameter of
    /// a marked name has both copies redacted. It gets a duplicate parameter on
    /// the wire, though, which the venue resolves by its own rule — so don't:
    /// pass the credential here and nowhere else.
    ///
    /// Unlike a header, a query parameter needs no validation: `reqwest`
    /// percent-encodes name and value when it builds the URL, so there is no
    /// malformed-input case to reject and no `Result` to return.
    pub fn with_secret_query_param(mut self, name: &str, value: &str) -> Self {
        self.secret_query.push(SecretParam {
            name: name.to_string(),
            value: value.to_string(),
        });
        self
    }

    /// Replace every credential query-parameter value in a transport error's
    /// URL with [`REDACTED`], leaving the rest of the error intact.
    ///
    /// `reqwest::Error::url_mut` reaches the very field the error's `Display`
    /// renders, so rewriting the query here is what keeps the key out of the
    /// log. A `.with_context` cannot do this job: this crate's own contexts are
    /// already clean — [`HttpClient::get_json`] builds them from base plus path
    /// and applies the query separately on the `RequestBuilder` — so the
    /// exposure lives entirely in the *source* error's own render, which no
    /// context of ours wraps away.
    ///
    /// Values on [`BENIGN_QUERY_PARAMS`] stay legible, because they are what a
    /// failed paged backfill is diagnosed from: which symbol, which interval,
    /// which window. Every other value is replaced, registered as a credential
    /// or not.
    ///
    /// **There is deliberately no early return for a client that registered no
    /// credential.** The previous shape returned early when `secret_query` was
    /// empty, which was sound only while the deny-list was the mechanism — under
    /// default-deny that client is precisely the one at risk, since a key passed
    /// through `get_json`'s `query` registers nothing. Restoring the early
    /// return would reopen the hole this inversion closes.
    ///
    /// **The rewrite re-normalizes the query, which is now visible on every
    /// venue.** `query_pairs()` decodes and `extend_pairs` re-serializes as
    /// `application/x-www-form-urlencoded`, so a rendered URL comes back
    /// equivalent but not byte-identical: `ids[]` reads as `ids%5B%5D`, a
    /// literal space as `+`, and a valueless `?foo` as `foo=`. Repeated keys
    /// and their order survive intact. Under the deny-list this pass ran only
    /// for the two keyed adapters; it now runs for all of them, so the cosmetic
    /// difference shows up in diagnostics that used to pass through untouched.
    /// It is noted here so the next reader does not chase it as a bug.
    fn redact_query(&self, mut err: reqwest::Error) -> reqwest::Error {
        if let Some(url) = err.url_mut() {
            // Reachable now that any client's error passes through here: a
            // venue whose request carries no query at all lands on this guard.
            // Clearing an absent query renders a bare trailing `?`, so the
            // check is what keeps such a URL intact.
            if url.query().is_some() {
                let redacted: Vec<(String, String)> = url
                    .query_pairs()
                    .map(|(name, value)| {
                        // Benign **and** not registered as a credential — the
                        // two lists compose rather than one shadowing the
                        // other. Registering a name through
                        // `with_secret_query_param` has to redact it
                        // unconditionally, so that a later edit adding that
                        // same name to the allow-list cannot silently render a
                        // key in clear. Testing the allow-list alone would make
                        // an explicit registration quietly ineffective, which
                        // is the opposite of what registering it says.
                        let keep = BENIGN_QUERY_PARAMS.contains(&name.as_ref())
                            && !self
                                .secret_query
                                .iter()
                                .any(|marked| marked.name == name.as_ref());
                        let value = if keep {
                            value.into_owned()
                        } else {
                            REDACTED.to_string()
                        };
                        (name.into_owned(), value)
                    })
                    .collect();
                url.query_pairs_mut().clear().extend_pairs(&redacted);
            }
        }
        err
    }

    /// Raise this source's minimum interval above [`DEFAULT_MIN_INTERVAL`] —
    /// the seam for a venue whose keyless tier is stricter than the default
    /// floor. Most venues need it; docs/data-feeds.md §10 tabulates every
    /// venue's documented limit and the floor derived from it.
    ///
    /// **This bounds a rate. It cannot bound a quota — do not read an interval
    /// as a budget guarantee.** This is the canonical statement of that
    /// distinction; the adapters point here rather than restating it. The gate
    /// is in-process state: it paces requests while the process is up and resets
    /// when the process does. So an interval chosen to satisfy a *per-day* or
    /// *per-month* allowance holds only across one continuous run, and a
    /// crash-loop — or a few local stack cycles in an afternoon — exhausts the
    /// allowance while every individual pacing decision here stays correct. The
    /// gap is invisible precisely because the steady-state arithmetic checks
    /// out. Holding a quota needs durable state (a persisted per-venue counter),
    /// which this client does not have; where a venue offers a route priced as a
    /// rate rather than a quota, preferring it removes the exposure instead of
    /// managing it.
    ///
    /// **And it bounds a rate *per process*, which is narrower than most venue
    /// limits.** A keyless tier is typically metered per IP, and this gate
    /// cannot see across process boundaries — so N processes on one host get N
    /// floors against one budget. Clones of a client *do* share one gate (see
    /// the field docs on `next_allowed`), which is why a venue polled by several
    /// sources in one process wants one client cloned rather than several built.
    /// Note the asymmetry there: the gate is shared but `min_interval` is
    /// per-clone, so raising the floor on one clone does not bind its siblings.
    pub fn with_min_interval(mut self, interval: Duration) -> Self {
        self.min_interval = interval;
        self
    }

    /// Change this source's response-body cap from
    /// [`DEFAULT_MAX_RESPONSE_BYTES`] — for a venue whose legitimate payload is
    /// larger (a wide batched fetch, a long candle page).
    pub fn with_max_response_bytes(mut self, max: usize) -> Self {
        self.max_response_bytes = max;
        self
    }

    /// Claim the next request slot, returning the instant it may be issued at.
    /// The slot is consumed whether or not the request then succeeds — a failed
    /// request still spent venue quota.
    fn reserve(&self) -> Instant {
        let now = Instant::now();
        let mut gate = self
            .next_allowed
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let at = gate.map_or(now, |queued| queued.max(now));
        *gate = Some(at + self.min_interval);
        at
    }

    /// Hold every later request off for `wait`, never pulling in a cooldown
    /// another caller has already set further out. A slot another caller
    /// reserved before the 429 landed still goes out — the runner polls a
    /// source sequentially, so that window is theoretical, and the cost if it
    /// opens is one extra request, not a lost cooldown.
    fn cool_down(&self, wait: Duration) {
        let resume = Instant::now() + wait;
        let mut gate = self
            .next_allowed
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *gate = Some(match *gate {
            Some(queued) if queued > resume => queued,
            _ => resume,
        });
    }

    /// GET `{base_url}{path}` with optional query params, decoding the JSON
    /// body into `T`. A non-success status is an error.
    ///
    /// The call waits out this source's minimum interval before going out. A
    /// 429 records the venue's `Retry-After` as a cooldown the next call waits
    /// through, and is surfaced as an error rather than retried here: the
    /// runner already logs it, reports it to metrics, and backs off, and a
    /// cooldown well past the request timeout does not belong inside one call.
    ///
    /// Any credential parameters set by
    /// [`HttpClient::with_secret_query_param`] are appended to `query`, and
    /// every `reqwest` error on the way out has those values redacted before it
    /// is wrapped. `url` below is deliberately base-plus-path with no query,
    /// which is what keeps this crate's own error contexts clean.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        // Borrowed as plain pairs for the serializer, which is all `.query`
        // needs — `SecretParam` deliberately implements no `Serialize`, so the
        // only way its value reaches a URL is right here.
        let secret_query: Vec<(&str, &str)> = self
            .secret_query
            .iter()
            .map(|param| (param.name.as_str(), param.value.as_str()))
            .collect();
        sleep_until(self.reserve()).await;
        let response = self
            .client
            .get(&url)
            .headers(self.headers.clone())
            .query(query)
            // Appended after the caller's params. An empty set is a no-op:
            // `reqwest` normalizes a resulting empty query back to none.
            .query(&secret_query)
            .send()
            .await
            .map_err(|err| self.redact_query(err))
            .with_context(|| format!("GET {url}"))?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let wait = retry_after(response.headers()).unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN);
            self.cool_down(wait);
            bail!(
                "GET {url} was rate limited (429); holding off {}s",
                wait.as_secs()
            );
        }
        // `error_for_status` below covers 4xx and 5xx only, so without this a
        // refused redirect would arrive as a 3xx with an empty body and surface
        // as a JSON decode error — loud, but pointing at the wrong thing. Say
        // what actually happened instead: `HttpClient::new` declines to follow
        // redirects on purpose, and an operator seeing this needs to know it is
        // a policy refusal, not a malformed venue response.
        //
        // Matched against the statuses `reqwest` would actually have followed,
        // not the whole 3xx class: 300, 304 and 305 are 3xx but are not
        // followable redirects, and telling an operator their 304 was "a
        // redirect this transport refuses to follow" would be a false
        // statement. Nothing here sends conditional-request headers today, so
        // no 304 is expected — but `with_header` is a public seam, and the
        // message has to stay true the day someone adds `If-None-Match`.
        if REFUSED_REDIRECTS.contains(&response.status().as_u16()) {
            bail!(
                "GET {url} answered {} — a redirect, which this transport \
                 refuses to follow so a credential cannot be replayed to \
                 another host",
                response.status()
            );
        }
        let response = response
            .error_for_status()
            .map_err(|err| self.redact_query(err))
            .with_context(|| format!("GET {url} returned an error status"))?;
        let body = self.read_capped(response, &url).await?;
        serde_json::from_slice(&body).with_context(|| format!("decode JSON from {url}"))
    }

    /// Buffer the body, refusing one that outruns the cap. A declared
    /// `Content-Length` is checked first so an oversized response costs nothing
    /// to reject; the running total then covers a venue that under-declares or
    /// omits it entirely.
    async fn read_capped(&self, mut response: reqwest::Response, url: &str) -> Result<Vec<u8>> {
        let cap = self.max_response_bytes;
        if let Some(len) = response.content_length() {
            if len > cap as u64 {
                bail!("{url} declared a {len}-byte body, over the {cap}-byte cap");
            }
        }
        // The running total is what covers a venue that under-declares its
        // length or omits it entirely. It has no unit test: a `reqwest::Response`
        // built in-process always reports a truthful `content_length`, so the
        // check above fires first and this branch is only reachable against a
        // real streaming venue.
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            // A body error carries no URL today, so this is belt-and-braces —
            // but the guarantee should not rest on which errors `reqwest`
            // happens to attach a URL to.
            .map_err(|err| self.redact_query(err))
            .with_context(|| format!("read body from {url}"))?
        {
            if body.len() + chunk.len() > cap {
                bail!("{url} sent a body over the {cap}-byte cap");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// The `Retry-After` delay a 429 carries, clamped to
/// [`MAX_RATE_LIMIT_COOLDOWN`]. Only the delta-seconds form is read — the
/// HTTP-date form would need a date parser this crate does not depend on, and
/// falls through to the default cooldown instead.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds: u64 = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds).min(MAX_RATE_LIMIT_COOLDOWN))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_after_headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn with_header_accepts_a_valid_pair_and_rejects_a_malformed_name() {
        // A valid benign header composes onto the client. (A credential would
        // go through `with_secret_header` instead.)
        let ok = HttpClient::new("https://example.test")
            .unwrap()
            .with_header("Accept-Datetime-Format", "UNIX");
        assert!(ok.is_ok());
        // A space is not legal in a header name — caught at wiring time, not on
        // the first (network) request.
        let bad = HttpClient::new("https://example.test")
            .unwrap()
            .with_header("bad name", "v");
        assert!(bad.is_err());
    }

    #[test]
    fn with_secret_header_marks_the_value_sensitive_and_keeps_it_out_of_debug() {
        let keyed = HttpClient::new("https://example.test")
            .unwrap()
            .with_secret_header("X-CMC_PRO_API_KEY", "super-secret-key")
            .unwrap();
        let value = keyed.headers.get("X-CMC_PRO_API_KEY").unwrap();
        assert!(value.is_sensitive());
        // The invariant that matters: a `Debug` render of the map — what any
        // later `#[derive(Debug)]` on the path would reach — shows a redaction
        // marker, not the key.
        let rendered = format!("{:?}", keyed.headers);
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
        // Assert the marker too, not just the absence: this pins the keyed half
        // on its own, so the negative above cannot quietly go vacuous if the
        // map's `Debug` ever stops rendering values at all.
        assert!(rendered.contains("Sensitive"), "{rendered}");

        // A benign header stays debug-visible on purpose: marking everything
        // sensitive would cost the diagnostics this one is kept for.
        let plain = HttpClient::new("https://example.test")
            .unwrap()
            .with_header("Accept-Datetime-Format", "UNIX")
            .unwrap();
        let plain_value = plain.headers.get("Accept-Datetime-Format").unwrap();
        assert!(!plain_value.is_sensitive());
        assert!(format!("{:?}", plain.headers).contains("UNIX"));
    }

    #[test]
    fn a_rejected_secret_value_does_not_echo_itself_in_the_error() {
        // A newline is illegal in a header value. The error names the header so
        // the mistake is diagnosable, and omits the value so the credential does
        // not ride the error path into a log.
        // `.err()` rather than `unwrap_err()`: the latter would require
        // `HttpClient: Debug`, which this type deliberately does not derive —
        // the very habit `with_secret_header` refuses to depend on.
        let err = HttpClient::new("https://example.test")
            .unwrap()
            .with_secret_header("Authorization", "Bearer super-secret-token\ntail")
            .err()
            .expect("a newline in a header value is rejected");
        let rendered = format!("{err:?}");
        assert!(rendered.contains("Authorization"), "{rendered}");
        // Assert on the whole distinctive phrase, not a short fragment of it: a
        // three-character needle would risk a false failure the day the error
        // chain happens to render an unrelated string containing it.
        assert!(!rendered.contains("super-secret-token"), "{rendered}");
    }

    /// A client aimed at a loopback port nothing listens on. The connection is
    /// refused immediately, which is enough to reach the error path the
    /// credential exposure rides — no network and no mock server. Should a
    /// platform ever hang instead of refusing, the request timeout produces a
    /// `TimedOut` request error, and `reqwest` attaches the effective URL to
    /// that one too, so either outcome exercises the same assertion.
    fn refusing_client() -> HttpClient {
        HttpClient::new("http://127.0.0.1:1").unwrap()
    }

    #[tokio::test]
    async fn a_credential_query_param_is_redacted_out_of_a_request_error() {
        let client = refusing_client().with_secret_query_param("apikey", "super-secret-key");
        let err = client
            .get_json::<serde_json::Value>("/query", &[("from_symbol", "AUD")])
            .await
            .expect_err("a refused connection is an error");
        // The invariant: the key is absent from the `anyhow` chain a top-level
        // handler logs. `{err:?}` is what such a handler renders, and it is the
        // render that reaches the source error's own `Display` — where the
        // effective URL, query included, actually lives.
        let rendered = format!("{err:?}");
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
        // Assert the marker too, so the negative above cannot quietly go vacuous
        // the day the URL stops being rendered at all.
        assert!(rendered.contains("apikey=REDACTED"), "{rendered}");
        // And the benign parameter survives, which is the whole reason the
        // transport marks one parameter instead of stripping every query: a
        // failed paged backfill is diagnosed from exactly these.
        assert!(rendered.contains("from_symbol=AUD"), "{rendered}");
    }

    #[tokio::test]
    async fn a_client_with_no_credential_param_keeps_its_whole_query() {
        // The counterpart to the test above: a venue with no credential
        // parameter loses nothing from its diagnostics, because every parameter
        // it sends is on `BENIGN_QUERY_PARAMS`. That is the half of default-deny
        // worth pinning — the inversion is only affordable if ordinary backfill
        // diagnostics stay legible.
        //
        // Be exact about the reach, because the comment this replaced was
        // scrupulous about it and the first rewrite was not: this fails if
        // `granularity` is dropped from the allow-list, and only that name.
        // Every other entry is covered by
        // `every_wired_adapter_parameter_is_on_the_benign_list` below, which
        // walks the whole set.
        let err = refusing_client()
            .get_json::<serde_json::Value>("/products/EURC-USDC/candles", &[("granularity", "60")])
            .await
            .expect_err("a refused connection is an error");
        let rendered = format!("{err:?}");
        assert!(rendered.contains("granularity=60"), "{rendered}");
        assert!(!rendered.contains("REDACTED"), "{rendered}");
    }

    #[tokio::test]
    async fn an_unregistered_credential_is_redacted_by_default_deny() {
        // The hole default-deny closes, and the reason this inversion is
        // load-bearing rather than belt-and-braces. This client registers no
        // credential at all and hand-passes one through `get_json`'s query —
        // what an adapter author does by mistake, since nothing forces the
        // constructor route. Under the deny-list this rendered the key in
        // clear, and a transport error's text reaches the feed-health
        // last-error column, which the read-only dashboard role can select and
        // the operations panel renders verbatim.
        let err = refusing_client()
            .get_json::<serde_json::Value>("/query", &[("token", "super-secret-key")])
            .await
            .expect_err("a refused connection is an error");
        let rendered = format!("{err:?}");
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
        // The name still renders, so the redaction stays diagnosable — the same
        // property the registered-credential path is pinned on.
        assert!(rendered.contains("token=REDACTED"), "{rendered}");
    }

    #[test]
    fn no_credential_name_is_on_the_benign_allow_list() {
        // The allow-list is now the whole of the redaction, so a credential
        // name landing on it would reopen the hole for every client at once —
        // silently, and without touching `redact_query`. `apikey` is what both
        // wired keyed adapters authenticate with (Alpha Vantage and Twelve
        // Data); the rest are the spellings someone is most likely to reach for
        // when wiring the next keyed venue, which is exactly when this list
        // gets edited.
        for name in [
            "access_key",
            "access_token",
            "api_key",
            "apikey",
            "app_key",
            "appid",
            "auth",
            "client_secret",
            "key",
            "passwd",
            "password",
            "secret",
            "session",
            "sig",
            "signature",
            "token",
        ] {
            assert!(
                !BENIGN_QUERY_PARAMS.contains(&name),
                "`{name}` must never be treated as a benign query parameter"
            );
        }
    }

    #[tokio::test]
    async fn a_registered_credential_is_redacted_even_if_its_name_looks_benign() {
        // The allow-list and the registration list **compose**; neither
        // shadows the other. Registering a name through
        // `with_secret_query_param` is a statement that the value is a
        // credential, so it must be redacted whatever the allow-list says —
        // otherwise one careless addition to `BENIGN_QUERY_PARAMS` silently
        // renders a live key in clear for every client that registered it.
        //
        // `symbol` is used as the stand-in precisely because it *is* benign
        // and *is* on the list: testing with a name the list already rejects
        // would pass without the composition being there at all.
        let client = refusing_client().with_secret_query_param("symbol", "super-secret-key");
        let err = client
            .get_json::<serde_json::Value>("/query", &[("granularity", "60")])
            .await
            .expect_err("a refused connection is an error");
        let rendered = format!("{err:?}");
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
        assert!(rendered.contains("symbol=REDACTED"), "{rendered}");
        // And the genuinely benign parameter beside it still renders, so the
        // composition did not collapse into blanket redaction.
        assert!(rendered.contains("granularity=60"), "{rendered}");
    }

    #[test]
    fn every_wired_adapter_parameter_is_on_the_benign_list() {
        // The allow-list is only affordable if it actually covers what the
        // wired adapters send: a name missing from it renders as `REDACTED`
        // and costs a round of diagnosis on a failed backfill. The
        // single-parameter test above reaches exactly one entry, so this is
        // what pins the rest — and it fails when a new venue lands without its
        // benign parameters being added, which is the moment the omission is
        // cheapest to fix.
        //
        // Grouped by the adapter that sends them, so a venue removed from the
        // tree takes its row out with it rather than leaving an orphan.
        let wired: &[(&str, &[&str])] = &[
            (
                "alphavantage",
                &["function", "from_symbol", "to_symbol", "outputsize"],
            ),
            ("coinbase", &["granularity", "start", "end"]),
            ("coingecko", &["ids", "vs_currencies"]),
            ("coinmarketcap", &["ids"]),
            ("frankfurter", &["base", "symbols"]),
            ("kraken", &["pair"]),
            ("oanda", &["granularity", "from", "to", "price"]),
            ("pyth", &["ids[]", "parsed"]),
            (
                "twelvedata",
                &["symbol", "interval", "timezone", "start_date", "end_date"],
            ),
        ];
        for (venue, params) in wired {
            for param in *params {
                assert!(
                    BENIGN_QUERY_PARAMS.contains(param),
                    "`{venue}` sends `{param}`, which is not on BENIGN_QUERY_PARAMS — \
                     its value will render as REDACTED in every transport error"
                );
            }
        }
    }

    #[test]
    fn the_benign_allow_list_is_sorted_and_free_of_duplicates() {
        // Not style policing: the list is edited by hand every time a venue
        // lands, and an unsorted list is where a duplicate — or a second,
        // divergent spelling of a name already present — hides. Sorted order is
        // what makes a review of that edit a local read rather than a scan.
        let mut sorted = BENIGN_QUERY_PARAMS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.as_slice(), BENIGN_QUERY_PARAMS);
    }

    /// Answer one request on loopback with `response`, returning the port to
    /// aim a client at.
    ///
    /// The request head is **drained before answering**, which is load-bearing
    /// rather than tidy: closing a socket that still holds unread received data
    /// sends RST instead of FIN on both Darwin and Linux, and a RST that
    /// overtakes the response surfaces as a connection reset instead of the
    /// status under test. That is the shape of a test which passes locally and
    /// fails once a month in the merge queue.
    async fn serve_once(response: &'static [u8]) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).await.unwrap() == 0 {
                    break;
                }
                head.extend_from_slice(&byte);
            }
            socket.write_all(response).await.unwrap();
        });
        port
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        // A cross-host 302. If the client followed it, the hop would carry every
        // configured header — including a credential `reqwest` does not strip,
        // since its cross-host strip list is by header name and never consults
        // the sensitive marking. So this pins the policy, not just the message.
        let port = serve_once(
            b"HTTP/1.1 302 Found\r\n\
              Location: http://credential-thief.invalid/\r\n\
              Content-Length: 0\r\n\r\n",
        )
        .await;

        // A custom-named key header — deliberately not `Authorization`, which
        // `reqwest` would strip on its own. This is the shape the policy exists
        // for: no adapter carries one today, and the guarantee has to hold for
        // the one that eventually does.
        let client = HttpClient::new(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .with_secret_header("X-Venue-Api-Key", "super-secret-key")
            .unwrap();
        let err = client
            .get_json::<serde_json::Value>("/v1/quotes", &[])
            .await
            .expect_err("a refused redirect is an error");

        // The 302 is surfaced as itself. Without the explicit redirection check
        // this would instead be a JSON decode error over the empty body — still
        // an error, but one that misdiagnoses a policy refusal as a malformed
        // venue response.
        let rendered = format!("{err:?}");
        assert!(rendered.contains("302"), "{rendered}");
        assert!(rendered.contains("refuses to follow"), "{rendered}");
        // The redirect target is never contacted, so it cannot appear as the
        // failing URL — the way it would if the hop had been followed and then
        // failed to resolve.
        assert!(!rendered.contains("credential-thief.invalid"), "{rendered}");
        // The header credential is configured above precisely so this assertion
        // can exist: whatever the transport reports about a refused redirect, it
        // must not carry the key it declined to replay.
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
    }

    #[tokio::test]
    async fn a_credential_query_param_is_redacted_out_of_an_error_status() {
        // The `error_for_status` path, which is the one a real keyed venue
        // actually hits — Alpha Vantage and Twelve Data both answer an
        // unauthorized key with a 401 whose effective URL carries `apikey`.
        // The refused-connection test above covers only the `send` path, so
        // without this the redaction that matters most in production is the
        // one with no coverage.
        let port = serve_once(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n").await;

        let err = HttpClient::new(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .with_secret_query_param("apikey", "super-secret-key")
            .get_json::<serde_json::Value>("/time_series", &[("symbol", "AUD/USD")])
            .await
            .expect_err("a 401 is an error");

        let rendered = format!("{err:?}");
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
        assert!(rendered.contains("apikey=REDACTED"), "{rendered}");
        // `/` percent-encodes to %2F on the round-trip through `query_pairs`,
        // which is the faithful form — asserting it pins that a benign value
        // survives re-encoding rather than being mangled.
        assert!(rendered.contains("symbol=AUD%2FUSD"), "{rendered}");
    }

    #[test]
    fn a_secret_query_param_keeps_its_value_out_of_debug() {
        // The query-param counterpart to
        // `with_secret_header_marks_the_value_sensitive_and_keeps_it_out_of_debug`.
        // A query value is not a `HeaderValue`, so `set_sensitive` cannot reach
        // it; the protection has to live in the type, and this is what stops the
        // guarantee from depending on which types happen not to derive `Debug`.
        let param = SecretParam {
            name: "apikey".to_string(),
            value: "super-secret-key".to_string(),
        };
        let rendered = format!("{param:?}");
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
        // The name still renders, so a redacted param is diagnosable.
        assert_eq!(rendered, "apikey=REDACTED");
    }

    #[test]
    fn retry_after_reads_delta_seconds_and_clamps_a_hostile_delay() {
        assert_eq!(
            retry_after(&retry_after_headers("30")),
            Some(Duration::from_secs(30))
        );
        // Surrounding whitespace is legal in a header value.
        assert_eq!(
            retry_after(&retry_after_headers(" 12 ")),
            Some(Duration::from_secs(12))
        );
        // A venue asking for a day is held to the ceiling, so the feed surfaces
        // the problem instead of going quiet for hours.
        assert_eq!(
            retry_after(&retry_after_headers("86400")),
            Some(MAX_RATE_LIMIT_COOLDOWN)
        );
    }

    #[test]
    fn retry_after_falls_through_on_a_missing_or_malformed_value() {
        assert_eq!(retry_after(&HeaderMap::new()), None);
        // The HTTP-date form is legal but unread; the caller's default applies.
        assert_eq!(
            retry_after(&retry_after_headers("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reserve_spaces_back_to_back_slots_by_the_minimum_interval() {
        let client = HttpClient::new("https://example.test")
            .unwrap()
            .with_min_interval(Duration::from_millis(200));
        // The first request goes out immediately; each later one is pushed a
        // full interval past the one before it.
        let first = client.reserve();
        let second = client.reserve();
        let third = client.reserve();
        assert_eq!(second - first, Duration::from_millis(200));
        assert_eq!(third - second, Duration::from_millis(200));
    }

    #[tokio::test(start_paused = true)]
    async fn reserve_does_not_bank_credit_while_a_client_sits_idle() {
        let client = HttpClient::new("https://example.test")
            .unwrap()
            .with_min_interval(Duration::from_millis(200));
        let first = client.reserve();
        sleep_until(first + Duration::from_secs(5)).await;
        // An idle stretch does not earn a burst: the next slot is now, not five
        // seconds' worth of skipped slots ago.
        assert_eq!(client.reserve(), Instant::now());
    }

    #[tokio::test(start_paused = true)]
    async fn a_clone_draws_on_the_same_budget() {
        let client = HttpClient::new("https://example.test")
            .unwrap()
            .with_min_interval(Duration::from_millis(200));
        let clone = client.clone();
        let first = client.reserve();
        // The clone is the same source, so its slot queues behind the original's
        // rather than opening a second budget.
        assert_eq!(clone.reserve() - first, Duration::from_millis(200));
    }

    /// A response built in-process, so the body cap is exercised without a live
    /// venue or a mock server.
    fn response_with_body(body: Vec<u8>) -> reqwest::Response {
        let builder = http::Response::builder().header(http::header::CONTENT_LENGTH, body.len());
        reqwest::Response::from(builder.body(body).unwrap())
    }

    #[tokio::test]
    async fn read_capped_refuses_an_oversized_body() {
        let client = HttpClient::new("https://example.test")
            .unwrap()
            .with_max_response_bytes(16);
        // A venue that answers with far more than the consumer will hold is
        // refused rather than allocated for.
        let err = client
            .read_capped(response_with_body(vec![b'x'; 64]), "url")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("declared a 64-byte body"), "{err}");
    }

    #[tokio::test]
    async fn read_capped_returns_a_body_within_the_cap() {
        let client = HttpClient::new("https://example.test").unwrap();
        let body = client
            .read_capped(response_with_body(b"{\"ok\":true}".to_vec()), "url")
            .await
            .unwrap();
        assert_eq!(body, b"{\"ok\":true}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_cooldown_holds_the_next_slot_and_never_shortens_a_longer_one() {
        let client = HttpClient::new("https://example.test").unwrap();
        let now = Instant::now();
        client.cool_down(Duration::from_secs(60));
        assert_eq!(client.reserve(), now + Duration::from_secs(60));
        // A shorter cooldown landing while a longer one is in force leaves the
        // longer one standing.
        client.cool_down(Duration::from_secs(90));
        client.cool_down(Duration::from_secs(5));
        assert_eq!(client.reserve(), now + Duration::from_secs(90));
    }
}
