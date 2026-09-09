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
/// The receiver resolves once the head has been drained. It yields an `Err` if
/// the client hung up before completing one, which a caller may treat as a
/// failure or ignore.
pub(crate) async fn serve_once_capturing(response: Vec<u8>) -> (u16, oneshot::Receiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = oneshot::channel();
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
        // Sent before the response is written, so a caller that awaits the head
        // cannot deadlock against a client still waiting to be answered.
        let _ = tx.send(String::from_utf8_lossy(&head).into_owned());
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
