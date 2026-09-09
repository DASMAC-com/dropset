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
