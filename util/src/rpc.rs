//! RPC-endpoint derivation shared by the fill-subscription paths.

/// Derive the PubSub websocket endpoint from an RPC URL, matching the Agave
/// convention: swap the scheme (`http`→`ws`, `https`→`wss`) and use the RPC
/// port + 1 (the validator serves logs/account subscriptions there, so
/// `8899` → `8900`). Returns the input unchanged for an unrecognized scheme
/// (assume it is already a ws endpoint) or a non-numeric port.
pub fn ws_url_from_rpc(rpc_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = rpc_url.strip_prefix("https://") {
        ("wss://", rest)
    } else if let Some(rest) = rpc_url.strip_prefix("http://") {
        ("ws://", rest)
    } else {
        return rpc_url.to_string();
    };
    // PubSub lives at the root, so drop any path and bump the port.
    let authority = rest.split('/').next().unwrap_or(rest);
    let ws_authority = match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => format!("{host}:{}", port.saturating_add(1)),
            Err(_) => authority.to_string(),
        },
        None => authority.to_string(),
    };
    format!("{scheme}{ws_authority}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The websocket endpoint swaps the scheme and uses the RPC port + 1.
    #[test]
    fn ws_url_follows_the_agave_convention() {
        assert_eq!(
            ws_url_from_rpc("http://127.0.0.1:8899"),
            "ws://127.0.0.1:8900"
        );
        assert_eq!(
            ws_url_from_rpc("https://api.example.com:443/rpc"),
            "wss://api.example.com:444"
        );
        // Unrecognized scheme is assumed to already be a ws endpoint.
        assert_eq!(ws_url_from_rpc("ws://host:9000"), "ws://host:9000");
    }
}
