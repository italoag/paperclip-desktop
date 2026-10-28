//! Remote URL normalisation. Port of `src/connection/validate.ts`.
//!
//! Anything the user types goes through here before it is stored, probed, or
//! loaded into a webview, so the rules are deliberately strict: only HTTP(S), no
//! embedded credentials, and everything below the origin is discarded.

use std::net::{Ipv4Addr, Ipv6Addr};

use url::Url;

const PRIVATE_HOST_SUFFIXES: [&str; 5] = [".internal", ".local", ".lan", ".home", ".ts.net"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RemoteUrlError {
    #[error("Enter a valid remote URL.")]
    Invalid,

    #[error("Remote URLs must use HTTP or HTTPS.")]
    UnsupportedScheme,

    #[error("Remote URLs cannot include username or password.")]
    EmbeddedCredentials,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRemoteUrl {
    pub input: String,
    pub normalized_url: String,
    pub origin: String,
    pub insecure_transport: bool,
    pub warning: Option<String>,
}

pub fn normalize_remote_url(input: &str) -> Result<NormalizedRemoteUrl, RemoteUrlError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(RemoteUrlError::Invalid);
    }

    let mut parsed = Url::parse(trimmed).map_err(|_| RemoteUrlError::Invalid)?;

    let insecure_transport = match parsed.scheme() {
        "http" => true,
        "https" => false,
        _ => return Err(RemoteUrlError::UnsupportedScheme),
    };

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(RemoteUrlError::EmbeddedCredentials);
    }

    parsed.set_path("/");
    parsed.set_query(None);
    parsed.set_fragment(None);

    let host = parsed.host_str().unwrap_or_default().to_string();

    Ok(NormalizedRemoteUrl {
        input: trimmed.to_string(),
        normalized_url: parsed.to_string(),
        origin: parsed.origin().ascii_serialization(),
        insecure_transport,
        warning: insecure_transport.then(|| build_insecure_http_warning(&host)),
    })
}

pub fn is_private_hostname(hostname: &str) -> bool {
    let lower = normalize_hostname(hostname);

    if lower == "localhost" || lower == "::1" || lower.ends_with(".localhost") {
        return true;
    }

    if PRIVATE_HOST_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
    {
        return true;
    }

    is_private_ipv4(&lower) || is_private_ipv6(&lower)
}

fn is_private_ipv4(hostname: &str) -> bool {
    let Ok(ip) = hostname.parse::<Ipv4Addr>() else {
        return false;
    };
    let [a, b, ..] = ip.octets();

    a == 10
        || a == 127
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
        // Carrier-grade NAT, which is also the Tailscale range.
        || (a == 100 && (64..=127).contains(&b))
}

fn is_private_ipv6(hostname: &str) -> bool {
    if hostname == "::1" {
        return true;
    }
    if hostname.parse::<Ipv6Addr>().is_err() {
        return false;
    }
    // Unique local addresses, matching the fc00::/7 prefix test in the original.
    hostname.starts_with("fc") || hostname.starts_with("fd")
}

fn normalize_hostname(hostname: &str) -> String {
    let lower = hostname.to_lowercase();
    lower
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .map(str::to_string)
        .unwrap_or(lower)
}

fn build_insecure_http_warning(hostname: &str) -> String {
    const BASE: &str = "Warning: this remote uses HTTP without TLS. Network traffic can be read \
                        or modified by anyone on the path.";

    if is_private_hostname(hostname) {
        format!(
            "{BASE} Only continue if you trust the local or private network carrying this \
             connection."
        )
    } else {
        format!("{BASE} This host is not on a recognized private network, so the risk is higher.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_path_and_preserves_https_origin() {
        let normalized =
            normalize_remote_url("https://paperclip.example.com/app/dashboard?x=1#top").unwrap();

        assert_eq!(normalized.normalized_url, "https://paperclip.example.com/");
        assert_eq!(normalized.origin, "https://paperclip.example.com");
        assert!(!normalized.insecure_transport);
        assert_eq!(normalized.warning, None);
    }

    #[test]
    fn allows_http_remotes_and_attaches_an_insecurity_warning() {
        let ten_net = normalize_remote_url("http://10.0.1.25:3100").unwrap();
        assert_eq!(ten_net.normalized_url, "http://10.0.1.25:3100/");
        assert_eq!(ten_net.origin, "http://10.0.1.25:3100");
        assert!(ten_net.insecure_transport);
        assert!(ten_net.warning.unwrap().contains("without TLS"));

        let ip_address = normalize_remote_url("http://192.168.1.50:3100/dashboard").unwrap();
        assert_eq!(ip_address.normalized_url, "http://192.168.1.50:3100/");
        assert_eq!(ip_address.origin, "http://192.168.1.50:3100");

        let tailscale_ip = normalize_remote_url("http://100.100.100.100:3200").unwrap();
        assert_eq!(tailscale_ip.normalized_url, "http://100.100.100.100:3200/");
        assert_eq!(tailscale_ip.origin, "http://100.100.100.100:3200");

        let tailnet = normalize_remote_url("http://paperclip-host.tailnet.ts.net").unwrap();
        assert_eq!(
            tailnet.normalized_url,
            "http://paperclip-host.tailnet.ts.net/"
        );
        assert_eq!(tailnet.origin, "http://paperclip-host.tailnet.ts.net");
        assert!(tailnet
            .warning
            .unwrap()
            .contains("trust the local or private network"));

        let public_host = normalize_remote_url("http://paperclip.example.com").unwrap();
        assert_eq!(public_host.normalized_url, "http://paperclip.example.com/");
        assert!(public_host.warning.unwrap().contains("risk is higher"));
    }

    #[test]
    fn rejects_embedded_credentials() {
        assert_eq!(
            normalize_remote_url("https://user:pass@paperclip.example.com"),
            Err(RemoteUrlError::EmbeddedCredentials)
        );
    }

    #[test]
    fn rejects_non_http_schemes_and_blank_input() {
        assert_eq!(
            normalize_remote_url("ftp://paperclip.example.com"),
            Err(RemoteUrlError::UnsupportedScheme)
        );
        assert_eq!(normalize_remote_url("   "), Err(RemoteUrlError::Invalid));
        assert_eq!(
            normalize_remote_url("not a url"),
            Err(RemoteUrlError::Invalid)
        );
    }

    #[test]
    fn recognises_tailnet_and_rfc1918_hosts() {
        for private in [
            "paperclip-host.tailnet.ts.net",
            "10.0.1.25",
            "192.168.1.50",
            "100.100.100.100",
            "[::1]",
            "[fd00::1]",
            "localhost",
            "172.16.0.1",
            "169.254.1.1",
        ] {
            assert!(is_private_hostname(private), "{private} should be private");
        }

        for public in [
            "paperclip.example.com",
            "8.8.8.8",
            "172.32.0.1",
            "100.200.0.1",
        ] {
            assert!(!is_private_hostname(public), "{public} should be public");
        }
    }
}
