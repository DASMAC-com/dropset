//! Loopback HTTP stubs shared by this crate's tests.
//!
//! **Why this is a module rather than a helper in one test module.** The only
//! route into a poll [`Source`](crate::Source)'s `next` is HTTP, so a venue
//! adapter's poll-to-batch path is unreachable without a server to answer it.
//! While the crate's one stub lived private to `http.rs`'s own test module,
//! that path had no coverage in any venue: emptying the `Vec` a `next` wraps
//! its reading in compiled, passed every test, and would have shipped a
//! collector that polls forever writing nothing — the failure this repo has
//! already hit once for real. Promoting the stub is what makes the covering
//! test possible for every venue at once instead of copying a listener into
//! each one.
//!
//! Gated on `test` *and* `http`, deliberately: nothing here needs `reqwest`,
//! but every consumer does, and an ungated module would be dead code under a
//! `--no-default-features` test build, which CI compiles with `-D warnings`.

use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::oneshot;

/// Answer one request on loopback with `response`, returning the port to aim a
/// client at.
///
/// The request head is **drained before answering**, which is load-bearing
/// rather than tidy: closing a socket that still holds unread received data
/// sends RST instead of FIN on both Darwin and Linux, and a RST that overtakes
/// the response surfaces as a connection reset instead of the status under
/// test. That is the shape of a test which passes locally and fails once a
/// month in the merge queue.
pub(crate) async fn serve_once(response: Vec<u8>) -> u16 {
    serve_once_capturing(response).await.0
}

/// [`serve_once`], additionally handing back the request head the client sent.
///
/// The head is what lets a test assert the request a source actually *issued* —
/// its path and query — rather than only what it did with the answer. That
/// matters for an adapter whose two poll methods promise to ask the venue one
/// identical question and differ only in how much of the reply they decode: the
/// promise is invisible to a response-only assertion, so it would drift
/// silently.
///
/// **The captured head is the whole head, request headers included** — so
/// assert through [`request_line`] rather than on the raw string. A raw-head
/// assertion against a client built with `with_secret_header` would print the
/// credential into the CI log on failure.
///
/// Note the bound on that, because the obvious stronger claim is false:
/// narrowing to the request line removes a **header**-borne credential, not
/// every credential. `HttpClient` also carries secrets as **query
/// parameters** (`with_secret_query_param`), and a query string is part of the
/// request line — so a capturing test against such a client still has the
/// secret in what it asserts on. No test configures one today; a future one
/// must redact rather than rely on this helper.
///
/// The receiver resolves once a **complete** head has been drained, and yields
/// an `Err` if the client hung up before completing one — so a caller's
/// `expect` on it means what it says. Sending a partial head instead would
/// surface a truncated request as a confusing assertion failure on a mangled
/// request line, rather than as the disconnect it actually was. The one case
/// that neither resolves nor errors is a client that never connects at all,
/// which holds the sender inside a parked `accept`; in practice the client's
/// own failed request panics first.
pub(crate) async fn serve_once_capturing(response: Vec<u8>) -> (u16, oneshot::Receiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        let mut complete = true;
        while !head.ends_with(b"\r\n\r\n") {
            if socket.read(&mut byte).await.unwrap() == 0 {
                // EOF before the terminator: the client hung up mid-head.
                complete = false;
                break;
            }
            head.extend_from_slice(&byte);
        }
        // Sent before the response is written, so a caller that awaits the head
        // cannot deadlock against a client still waiting to be answered. A
        // partial head is dropped rather than sent, which is what makes the
        // `Err` contract above true — the receiver sees the disconnect.
        if complete {
            let _ = tx.send(String::from_utf8_lossy(&head).into_owned());
        }
        socket.write_all(&response).await.unwrap();
    });
    (port, rx)
}

/// Answer a **sequence** of requests on one port, capturing every request head.
///
/// [`serve_once_capturing`] covers an adapter whose poll is one request. It
/// cannot cover one whose poll *fans out* — Kraken's batch refusal falls through
/// to an isolation pass of one request per pair, and the behavior worth testing
/// is precisely the relationship between those requests and the batch that comes
/// after them. A per-request port cannot express that: the source holds one base
/// URL, and what needs asserting is that request N+1 changed because of what
/// request N answered.
///
/// **The heads come back through a shared `Vec` rather than a channel**, unlike
/// the single-shot helpers. A `oneshot` can only carry the batch after the last
/// response is written, so a caller that awaited it before issuing its requests
/// would deadlock — and with a sequence the caller cannot always know how many
/// requests its own source will make. Reading a `Mutex` after the polls have
/// returned has no such ordering hazard.
///
/// **Both SEQUENTIAL connection strategies are served**: reuse of one socket,
/// and a fresh socket per request once the previous one is closed. The outer loop
/// accepts a connection and the inner one keeps answering on it until the peer
/// hangs up. A delimited body is what *lets* a client keep the connection alive
/// — [`json_response`] sets a `Content-Length`, so reuse is permitted — but
/// which path reqwest actually takes is deliberately not relied on, and was not
/// measured.
///
/// **Connections are served strictly one at a time, so concurrency is NOT
/// served.** The inner loop blocks in `read` on the current socket, which means
/// `accept` is unreachable while a connection is open and idle: a client that
/// held one connection idle in its pool *while* opening a second would never have
/// the second accepted, and would stall to reqwest's request timeout. Nothing in
/// this crate does that — a `Source`'s `next` takes `&mut self`, and Kraken's
/// isolation pass is a sequential loop — but the limit is real, so a future
/// concurrent poll path needs a different stub rather than this one.
///
/// Requests past `responses.len()` are not answered — the task stops accepting,
/// so an extra request fails at the transport rather than hanging forever on a
/// server that has nothing left to say.
///
/// **The captured heads are whole heads, request headers included** — the same
/// hazard [`serve_once_capturing`] documents at length, and it applies here
/// unchanged: assert through [`request_line`], and note that narrowing to the
/// request line removes a *header*-borne credential but **not** a query-param
/// one, since the query string is part of the request line. `alphavantage` and
/// `twelvedata` both configure `with_secret_query_param` today, so a seam test
/// for either must redact rather than rely on this helper. Kraken, this helper's
/// only caller, is keyless.
pub(crate) async fn serve_sequence_capturing(
    responses: Vec<Vec<u8>>,
) -> (u16, Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&heads);
    tokio::spawn(async move {
        let mut sent = 0usize;
        while sent < responses.len() {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            while sent < responses.len() {
                // Drained a byte at a time for the same reason as the
                // single-shot helper: unread received data turns the socket
                // close into an RST that can overtake the response.
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                let mut complete = true;
                while !head.ends_with(b"\r\n\r\n") {
                    match socket.read(&mut byte).await {
                        Ok(0) | Err(_) => {
                            complete = false;
                            break;
                        }
                        Ok(_) => head.extend_from_slice(&byte),
                    }
                }
                // An incomplete head means this connection is done rather than
                // that the sequence is: fall back to `accept` for the next one,
                // without consuming a response on a request that never arrived.
                if !complete {
                    break;
                }
                // Captured BEFORE the response is written, which is load-bearing
                // rather than incidental: the caller reads these heads as soon as
                // its own requests have returned, so storing head N after
                // answering request N would race the client and could hand back a
                // short `Vec`.
                captured
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(String::from_utf8_lossy(&head).into_owned());
                if socket.write_all(&responses[sent]).await.is_err() {
                    // Roll the capture back, for the same reason the
                    // incomplete-head path above declines to consume a response:
                    // this request never received one, so leaving its head in
                    // place would shift every later head against the response it
                    // actually got.
                    captured
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .pop();
                    break;
                }
                sent += 1;
            }
        }
    });
    (port, heads)
}

/// A minimal `200 OK` carrying `body` as JSON.
///
/// `Content-Length` is set from the body rather than left to a connection
/// close, so the client reads a complete response instead of racing the
/// shutdown — the same reason the head is drained above.
pub(crate) fn json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// The request line — `GET /path?query HTTP/1.1` — out of a captured head.
pub(crate) fn request_line(head: &str) -> &str {
    head.lines().next().unwrap_or_default()
}
