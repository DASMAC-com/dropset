//! Solana Explorer deep links for localnet accounts.
//!
//! The explorer can point at an arbitrary RPC via `cluster=custom` +
//! `customUrl=<urlencoded rpc>`, so a localnet account opens in the same UI
//! as mainnet — survey accounts before a teardown, then watch them go
//! not-found after.

use solana_pubkey::Pubkey;

/// Build the explorer URL for `address` against the custom `rpc_url`
/// cluster.
pub fn account_url(rpc_url: &str, address: &Pubkey) -> String {
    format!(
        "https://explorer.solana.com/address/{address}?cluster=custom&customUrl={}",
        percent_encode(rpc_url)
    )
}

/// Open `address` in the system browser on the custom cluster.
pub fn open_account(rpc_url: &str, address: &Pubkey) -> std::io::Result<()> {
    open::that(account_url(rpc_url, address))
}

/// Percent-encode a string for use as a URL query-parameter value. Keeps
/// the RFC 3986 unreserved set (`A–Z a–z 0–9 - _ . ~`) and escapes
/// everything else — enough to encode `http://127.0.0.1:8899` correctly
/// without pulling in a urlencoding crate.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_local_rpc_url() {
        assert_eq!(
            percent_encode("http://127.0.0.1:8899"),
            "http%3A%2F%2F127.0.0.1%3A8899"
        );
    }

    #[test]
    fn builds_custom_cluster_url() {
        let addr = Pubkey::new_from_array([1u8; 32]);
        let url = account_url("http://127.0.0.1:8899", &addr);
        assert!(url.contains("cluster=custom"));
        assert!(url.contains("customUrl=http%3A%2F%2F127.0.0.1%3A8899"));
        assert!(url.contains(&addr.to_string()));
    }
}
