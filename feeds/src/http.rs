//! The HTTP-REST poll transport (`http` feature).

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
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

/// What a credential query parameter's value is rewritten to in an error URL.
/// A placeholder rather than a removal, so the error still shows that the
/// request *was* authenticated — a missing key and a rejected one are different
/// diagnoses.
const REDACTED: &str = "REDACTED";

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
    /// [`HttpClient::with_secret_query_param`], which is also what teaches
    /// [`HttpClient::redact_query`] which values to keep out of an error.
    secret_query: Vec<(String, String)>,
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
    /// what closes it: the client then knows which parameter is a credential,
    /// and `redact_query` strips its value out of every transport error before
    /// it is wrapped. An adapter that hand-passes a key through
    /// `get_json`'s `query` instead bypasses the mechanism — the same
    /// discipline `with_secret_header` asks for, and the reason both live on the
    /// constructor rather than at the call site.
    ///
    /// Unlike a header, a query parameter needs no validation: `reqwest`
    /// percent-encodes name and value when it builds the URL, so there is no
    /// malformed-input case to reject and no `Result` to return.
    pub fn with_secret_query_param(mut self, name: &str, value: &str) -> Self {
        self.secret_query
            .push((name.to_string(), value.to_string()));
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
    /// Only marked values are replaced. Every benign parameter stays legible,
    /// because they are what a failed paged backfill is diagnosed from: which
    /// symbol, which interval, which window.
    fn redact_query(&self, mut err: reqwest::Error) -> reqwest::Error {
        if self.secret_query.is_empty() {
            return err;
        }
        if let Some(url) = err.url_mut() {
            // Unreachable as called: past the early return there is at least
            // one credential parameter, and `get_json` appends it to every
            // request, so any URL reaching here carries a query. Kept because
            // the alternative on an unexpected input is a rendered URL with a
            // bare trailing `?`, and clearing an absent query is what produces
            // it.
            if url.query().is_some() {
                let redacted: Vec<(String, String)> = url
                    .query_pairs()
                    .map(|(name, value)| {
                        let secret = self
                            .secret_query
                            .iter()
                            .any(|(marked, _)| marked.as_str() == name.as_ref());
                        let value = if secret {
                            REDACTED.to_string()
                        } else {
                            value.into_owned()
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
        sleep_until(self.reserve()).await;
        let response = self
            .client
            .get(&url)
            .headers(self.headers.clone())
            .query(query)
            // Appended after the caller's params. An empty set is a no-op:
            // `reqwest` normalizes a resulting empty query back to none.
            .query(&self.secret_query)
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
        if response.status().is_redirection() {
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
        // parameter loses nothing from its diagnostics. This pins the early
        // return in `redact_query`, and with it the deliberate choice not to
        // blanket-redact.
        let err = refusing_client()
            .get_json::<serde_json::Value>("/products/EURC-USDC/candles", &[("granularity", "60")])
            .await
            .expect_err("a refused connection is an error");
        let rendered = format!("{err:?}");
        assert!(rendered.contains("granularity=60"), "{rendered}");
        assert!(!rendered.contains("REDACTED"), "{rendered}");
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        // A one-shot listener that answers whatever it is asked with a
        // cross-host 302. If the client followed it, the hop would carry every
        // configured header — including a credential `reqwest` does not strip,
        // since its cross-host strip list is by header name and never consults
        // the sensitive marking. So this pins the policy, not just the message.
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Answer without draining the request: the response is complete on
            // its own and the client is not streaming a body up.
            socket
                .write_all(
                    b"HTTP/1.1 302 Found\r\n\
                      Location: http://credential-thief.invalid/\r\n\
                      Content-Length: 0\r\n\r\n",
                )
                .await
                .unwrap();
        });

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
        server.await.unwrap();

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
