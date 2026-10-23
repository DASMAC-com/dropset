// cspell:word emudhra
// cspell:word SPKI
// cspell:word Sectigo
//! Proof that the Rust TLS stack refuses the distrusted eMudhra intermediates.
//!
//! Eight eMudhra intermediate CA keys are recoverable from their public
//! certificates, so a chain through any of them must not verify. Every Dropset
//! binary bundles its roots through rustls — none reads the OS bundle — which
//! makes the lockfile's `rustls-webpki` (its distrusted-SPKI list, checked on
//! leaves and trust anchors alike) and `webpki-roots` the whole fix. This test
//! verifies that from inside the runtime: it drives the crate's own
//! [`HttpClient`] (rustls, default roots) at the advisory's two probe hosts,
//! one chaining to an emSign root and one to the Sectigo cross-sign under AAA
//! Certificate Services, and asserts each handshake is refused **because the
//! chain is distrusted** — `webpki::Error::CertDistrusted`, the error the
//! distrust list raises. Any other failure fails the test: a DNS or connect
//! error, and equally an expired or misnamed probe certificate, would leave
//! the check green against a lock without the fix. The containers ship
//! binaries built from the same lock with the same compiled-in roots, so a
//! host run exercises the same trust path.
//!
//! It needs the network, so it is `#[ignore]`d, and `make test-postgres`
//! excludes its binary: a required check must not depend on a live
//! third-party host. Run it before the deploy ceremony with:
//!
//! ```sh
//! make tls-distrust-check
//! ```

#![cfg(feature = "http")]

use dropset_feeds::HttpClient;
use std::error::Error as StdError;

/// The advisory's probe hosts, each serving a leaf issued under a distrusted
/// eMudhra intermediate.
const PROBE_HOSTS: [&str; 2] = [
    "https://dv-g3b-emudhra.we-have-a-problem.com",
    "https://dv-g3b-aaa-emudhra.we-have-a-problem.com",
];

/// The rustls certificate error somewhere in `err`'s source chain, if any.
///
/// An `io::Error` wrapping a custom error reports that error's *source* as its
/// own, skipping the wrapped error itself — and that is exactly how the TLS
/// connector carries rustls's error — so each `io::Error` link is unwrapped by
/// hand rather than trusted to `source()`.
fn certificate_error<'a>(
    mut err: Option<&'a (dyn StdError + 'static)>,
) -> Option<&'a rustls::CertificateError> {
    while let Some(link) = err {
        if let Some(rustls::Error::InvalidCertificate(cert)) = link.downcast_ref() {
            return Some(cert);
        }
        if let Some(inner) = link
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
        {
            if let Some(found) = certificate_error(Some(inner)) {
                return Some(found);
            }
        }
        err = link.source();
    }
    None
}

/// Whether `cert` is the distrust list's refusal. rustls carries a
/// verifier-specific failure as `CertificateError::Other`, wrapping the
/// `webpki::Error` it came from.
fn is_distrusted(cert: &rustls::CertificateError) -> bool {
    match cert {
        rustls::CertificateError::Other(other) => matches!(
            other.0.downcast_ref::<webpki::Error>(),
            Some(webpki::Error::CertDistrusted)
        ),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "needs the network; run with `make tls-distrust-check`"]
async fn distrusted_emudhra_chains_are_refused() {
    for host in PROBE_HOSTS {
        let client = HttpClient::new(host).expect("build the feeds HTTP client");
        let err = match client.get_json::<serde_json::Value>("/", &[]).await {
            Ok(_) => panic!("{host} verified: its distrusted chain was accepted"),
            Err(err) => err,
        };
        let first: &(dyn StdError + 'static) = err.as_ref();
        match certificate_error(Some(first)) {
            Some(cert) if is_distrusted(cert) => println!("{host}: refused ({cert:?})"),
            Some(cert) => panic!(
                "{host} was refused for {cert:?}, not for distrust: that passes without the fix"
            ),
            // A status or decode error lands here too, and means the chain
            // verified: the handshake got far enough to read a response.
            None => panic!("{host} was not refused at the handshake: {err:#}"),
        }
    }
}
