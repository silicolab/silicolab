//! Provider-agnostic LLM transport layer.
//!
//! Pure transport + per-provider protocol: no `AppState`, no egui. The agent
//! loop (`frontend/agent`) depends only on [`types`] and the [`provider`] trait;
//! a provider adapter ([`anthropic`], [`openai_compat`]) translates to and from
//! vendor JSON entirely behind that boundary.

pub mod anthropic;
pub mod external;
pub mod openai_compat;
pub mod provider;
pub mod retry;
pub mod types;

/// Whether it is safe to send a bearer credential to `base_url`: HTTPS to any
/// host, or plaintext HTTP only to a loopback host (a local dev LLM server). Any
/// other `http://` would put the API key on the wire in the clear, so every
/// transport that sends the key — the completion call and the model-list fetch —
/// gates on this one check.
pub fn endpoint_is_safe(base_url: &str) -> bool {
    let lower = base_url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return true;
    }
    match lower.strip_prefix("http://") {
        Some(rest) => {
            let host = authority_host(rest);
            !host.is_empty() && host_is_loopback(host)
        }
        // No explicit http scheme: not a cleartext concern here — a malformed URL
        // fails when the request is built.
        None => true,
    }
}

/// The bare host of a URL authority (the part after `scheme://`): drop the
/// path/query/fragment, any `user[:pass]@` userinfo (the real host is after the
/// last `@`), the `:port`, and IPv6 brackets.
fn authority_host(after_scheme: &str) -> &str {
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    if let Some(inner) = host_port.strip_prefix('[') {
        return inner.split(']').next().unwrap_or(""); // [::1]:port -> ::1
    }
    host_port.split(':').next().unwrap_or("")
}

/// Whether `host` is `localhost` or a loopback IP. Parsing as an IP makes
/// `127.0.0.0/8` and `::1` correct while rejecting look-alikes like
/// `127.0.0.1.evil.com` that a textual `starts_with("127.")` test would accept.
fn host_is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::endpoint_is_safe;

    #[test]
    fn https_is_always_safe() {
        assert!(endpoint_is_safe("https://api.openai.com/v1"));
        assert!(endpoint_is_safe("HTTPS://API.OPENAI.COM/v1"));
    }

    #[test]
    fn http_is_safe_only_to_loopback() {
        assert!(endpoint_is_safe("http://localhost:11434/v1"));
        assert!(endpoint_is_safe("http://127.0.0.1:8080/v1"));
        assert!(endpoint_is_safe("http://127.0.0.5/v1")); // 127.0.0.0/8 is loopback
        assert!(endpoint_is_safe("http://[::1]:8080/v1"));
    }

    #[test]
    fn http_to_remote_is_rejected_including_loopback_lookalikes() {
        assert!(!endpoint_is_safe("http://api.example.com/v1"));
        assert!(!endpoint_is_safe("http://192.168.1.50:1234/v1"));
        // A hostname that merely starts with "127." is not loopback.
        assert!(!endpoint_is_safe("http://127.0.0.1.evil.com/v1"));
        assert!(!endpoint_is_safe("http://127.evil.com/v1"));
        // Userinfo must not be mistaken for the host.
        assert!(!endpoint_is_safe("http://127.0.0.1@evil.com/v1"));
        assert!(!endpoint_is_safe("http://localhost.evil.com/v1"));
        // 0.0.0.0 is not a loopback address.
        assert!(!endpoint_is_safe("http://0.0.0.0/v1"));
    }
}
