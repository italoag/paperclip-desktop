//! Origin policy and per-profile storage isolation.
//!
//! Port of `src/connection/window-policy.ts`. The navigation predicates are a
//! direct translation; the storage identifiers are not.
//!
//! # Partitions became data store identifiers
//!
//! Electron isolated each remote profile with a named session partition
//! (`persist:paperclip-remote-<id>`). Tauri/WKWebView has no such concept, so
//! isolation now runs through `WebviewWindowBuilder::data_store_identifier`,
//! which takes a raw 16-byte id (macOS 14+, hence the bumped
//! `minimumSystemVersion`).
//!
//! The ids are UUIDv5 values so they stay stable across launches — that is what
//! makes a saved remote profile keep its login — while remaining distinct per
//! profile. A phase-0 spike confirmed both properties against a live origin.

use url::Url;
use uuid::Uuid;

/// Distinguishes our ids from any other UUIDv5 the app might derive.
const PROFILE_NAMESPACE: &str = "paperclip-desktop:profile:";
const ORIGIN_NAMESPACE: &str = "paperclip-desktop:origin:";
const LOCAL_STORE: &str = "paperclip-desktop:local";

/// Only same-origin navigation stays inside the window.
pub fn is_navigation_allowed(target_url: &str, allowed_origin: &str) -> bool {
    match Url::parse(target_url) {
        Ok(parsed) => parsed.origin().ascii_serialization() == allowed_origin,
        Err(_) => false,
    }
}

/// Off-origin web links go to the user's browser; anything else is dropped.
pub fn should_open_externally(target_url: &str, allowed_origin: &str) -> bool {
    let Ok(parsed) = Url::parse(target_url) else {
        return false;
    };

    if parsed.origin().ascii_serialization() == allowed_origin {
        return false;
    }

    matches!(parsed.scheme(), "http" | "https")
}

/// Storage identity for a saved remote profile.
pub fn data_store_id_for_profile(profile_id: &str) -> [u8; 16] {
    derive(PROFILE_NAMESPACE, profile_id)
}

/// Storage identity for a remote the user has not saved yet.
///
/// Replaces the `sha256(origin)[..12]` partition key from `main.ts`.
pub fn data_store_id_for_origin(origin: &str) -> [u8; 16] {
    derive(ORIGIN_NAMESPACE, origin)
}

/// Storage identity for the embedded local server.
pub fn local_data_store_id() -> [u8; 16] {
    derive(LOCAL_STORE, "")
}

fn derive(namespace: &str, value: &str) -> [u8; 16] {
    *Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("{namespace}{value}").as_bytes(),
    )
    .as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allows_same_origin_navigation() {
        let allowed = "https://paperclip-host.tailnet.ts.net";
        assert!(is_navigation_allowed(
            "https://paperclip-host.tailnet.ts.net/dashboard",
            allowed
        ));
        assert!(!is_navigation_allowed("https://example.com", allowed));
        // A different port is a different origin.
        assert!(!is_navigation_allowed(
            "https://paperclip-host.tailnet.ts.net:8443/",
            allowed
        ));
        assert!(!is_navigation_allowed("not a url", allowed));
    }

    #[test]
    fn opens_external_http_links_outside_the_allowed_origin() {
        let allowed = "http://localhost:3100";
        assert!(should_open_externally(
            "https://docs.paperclip.ing",
            allowed
        ));
        assert!(!should_open_externally(
            "http://localhost:3100/settings",
            allowed
        ));
        assert!(!should_open_externally("mailto:test@example.com", allowed));
        assert!(!should_open_externally("javascript:alert(1)", allowed));
        assert!(!should_open_externally("file:///etc/passwd", allowed));
    }

    #[test]
    fn profile_store_ids_are_stable_and_distinct() {
        assert_eq!(
            data_store_id_for_profile("abc123"),
            data_store_id_for_profile("abc123"),
            "the same profile must keep its session across launches"
        );
        assert_ne!(
            data_store_id_for_profile("abc123"),
            data_store_id_for_profile("def456")
        );
    }

    /// Two remotes on the same host must not be able to see each other's login
    /// just because the ids were derived from overlapping strings.
    #[test]
    fn profile_origin_and_local_ids_never_collide() {
        let ids = [
            data_store_id_for_profile("example.com"),
            data_store_id_for_origin("example.com"),
            local_data_store_id(),
            data_store_id_for_profile(""),
        ];

        for (i, left) in ids.iter().enumerate() {
            for right in ids.iter().skip(i + 1) {
                assert_ne!(left, right);
            }
        }
    }

    #[test]
    fn local_store_id_is_constant() {
        assert_eq!(local_data_store_id(), local_data_store_id());
    }
}
