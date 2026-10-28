//! Remote verification performed before a remote is ever loaded in a window.
//!
//! Port of `src/connection/preflight.ts`. This is the security gate of the
//! remote-connection feature, so the logic is translated one-for-one:
//!
//! 1. the URL must be a credential-free HTTP(S) origin;
//! 2. `/api/health` must return a recognisable Paperclip payload over HTTP 200;
//! 3. the deployment must be `authenticated` — a loopback-only `local_trusted`
//!    server is rejected outright, since it trusts anyone who can reach it;
//! 4. its auth subsystem must be ready;
//! 5. `/api/auth/get-session` must answer in a way only Paperclip does.
//!
//! Anything unexpected fails closed.

use std::time::Duration;

use url::Url;

use super::http::HttpProbe;
use super::types::{
    BootstrapStatus, DeploymentExposure, DeploymentMode, RemotePreflightFailureReason,
    RemotePreflightResult, SessionState,
};
use super::validate::{normalize_remote_url, NormalizedRemoteUrl};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);

pub struct PreflightOptions<'a> {
    pub remote_url: &'a str,
    pub local_server_version: Option<&'a str>,
    pub timeout: Duration,
}

impl<'a> PreflightOptions<'a> {
    pub fn new(remote_url: &'a str) -> Self {
        Self {
            remote_url,
            local_server_version: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHealth {
    status: String,
    version: Option<String>,
    deployment_mode: DeploymentMode,
    deployment_exposure: Option<DeploymentExposure>,
    auth_ready: Option<bool>,
    bootstrap_status: Option<BootstrapStatus>,
    bootstrap_invite_active: Option<bool>,
}

pub async fn preflight_remote_connection<P: HttpProbe>(
    probe: &P,
    options: PreflightOptions<'_>,
) -> RemotePreflightResult {
    let normalized = match normalize_remote_url(options.remote_url) {
        Ok(normalized) => normalized,
        Err(err) => {
            return failure(
                options.remote_url.trim(),
                None,
                RemotePreflightFailureReason::InvalidUrl,
                err.to_string(),
                None,
            )
        }
    };

    let health_url = match join(&normalized.origin, "/api/health") {
        Ok(url) => url,
        Err(detail) => {
            return failure(
                &normalized.normalized_url,
                Some(&normalized),
                RemotePreflightFailureReason::InvalidUrl,
                detail,
                normalized.warning.clone(),
            )
        }
    };

    let health_response = match probe.get_json(health_url, options.timeout).await {
        Ok(response) => response,
        Err(err) => {
            return failure(
                &normalized.normalized_url,
                Some(&normalized),
                classify_probe_error(&err.message),
                err.message,
                normalized.warning.clone(),
            )
        }
    };

    let health = if health_response.status == 200 {
        health_response.body.as_ref().and_then(parse_health_payload)
    } else {
        None
    };

    let Some(health) = health else {
        return failure(
            &normalized.normalized_url,
            Some(&normalized),
            RemotePreflightFailureReason::NotPaperclip,
            "This host does not appear to expose the Paperclip health endpoint.",
            normalized.warning.clone(),
        );
    };

    let warning = build_version_warning(
        health.version.as_deref(),
        options.local_server_version,
        normalized.warning.as_deref(),
    );

    if health.status != "ok" {
        return detected(
            &normalized,
            &health,
            SessionState::Unknown,
            Some(RemotePreflightFailureReason::Unreachable),
            Some("Paperclip responded, but the instance is not healthy yet.".into()),
            warning,
        );
    }

    if health.deployment_mode == DeploymentMode::LocalTrusted {
        return detected(
            &normalized,
            &health,
            SessionState::Unknown,
            Some(RemotePreflightFailureReason::UnsupportedLocalTrusted),
            Some(
                "This Paperclip server is configured for loopback-only local use. Reconfigure it \
                 to upstream authenticated mode before using Desktop remote mode."
                    .into(),
            ),
            warning,
        );
    }

    if health.auth_ready == Some(false) {
        return detected(
            &normalized,
            &health,
            SessionState::Unknown,
            Some(RemotePreflightFailureReason::AuthNotReady),
            Some(
                "The remote reports authenticated mode, but its auth subsystem is not ready."
                    .into(),
            ),
            warning,
        );
    }

    let session_url = match join(&normalized.origin, "/api/auth/get-session") {
        Ok(url) => url,
        Err(detail) => {
            return failure(
                &normalized.normalized_url,
                Some(&normalized),
                RemotePreflightFailureReason::InvalidUrl,
                detail,
                normalized.warning.clone(),
            )
        }
    };

    let session_state = match probe.get_json(session_url, options.timeout).await {
        Ok(response) => parse_session_state(&response),
        Err(err) => {
            return failure(
                &normalized.normalized_url,
                Some(&normalized),
                classify_probe_error(&err.message),
                err.message,
                normalized.warning.clone(),
            )
        }
    };

    if session_state == SessionState::Unknown {
        return detected(
            &normalized,
            &health,
            SessionState::Unknown,
            Some(RemotePreflightFailureReason::NotPaperclip),
            Some("The remote session probe returned an unexpected response.".into()),
            warning,
        );
    }

    detected(&normalized, &health, session_state, None, None, warning)
}

/// Health payloads come in two shapes: the full one, and a redacted one that
/// public deployments serve to unauthenticated callers. Anything else is not
/// Paperclip.
fn parse_health_payload(body: &serde_json::Value) -> Option<ParsedHealth> {
    if !body.is_object() {
        return None;
    }

    let string = |key: &str| body.get(key).and_then(serde_json::Value::as_str);
    let boolean = |key: &str| body.get(key).and_then(serde_json::Value::as_bool);

    let deployment_mode = match string("deploymentMode") {
        Some("authenticated") => Some(DeploymentMode::Authenticated),
        Some("local_trusted") => Some(DeploymentMode::LocalTrusted),
        _ => None,
    };
    let deployment_exposure = match string("deploymentExposure") {
        Some("private") => Some(DeploymentExposure::Private),
        Some("public") => Some(DeploymentExposure::Public),
        _ => None,
    };
    let bootstrap_status = match string("bootstrapStatus") {
        Some("ready") => Some(BootstrapStatus::Ready),
        Some("bootstrap_pending") => Some(BootstrapStatus::BootstrapPending),
        _ => None,
    };
    let auth_ready = boolean("authReady");
    let bootstrap_invite_active = boolean("bootstrapInviteActive");
    let status = string("status");
    let version = string("version");

    let has_full_shape = deployment_exposure.is_some() && auth_ready.is_some();
    let has_redacted_authenticated_shape = status == Some("ok")
        && deployment_mode == Some(DeploymentMode::Authenticated)
        && bootstrap_status.is_some()
        && bootstrap_invite_active.is_some()
        && deployment_exposure.is_none()
        && auth_ready.is_none();

    let (Some(status), Some(deployment_mode)) = (status, deployment_mode) else {
        return None;
    };
    if !has_full_shape && !has_redacted_authenticated_shape {
        return None;
    }

    Some(ParsedHealth {
        status: status.to_string(),
        version: version.map(str::to_string),
        deployment_mode,
        deployment_exposure,
        auth_ready,
        bootstrap_status,
        bootstrap_invite_active,
    })
}

fn parse_session_state(response: &super::http::ProbeResponse) -> SessionState {
    let Some(body) = response.body.as_ref() else {
        return SessionState::Unknown;
    };

    if response.status == 401 {
        // Paperclip answers unauthenticated session probes with this exact
        // string; a generic 401 could come from any reverse proxy.
        let is_paperclip = body.get("error").and_then(serde_json::Value::as_str)
            == Some("Board authentication required");
        return if is_paperclip {
            SessionState::SignedOut
        } else {
            SessionState::Unknown
        };
    }

    if response.status != 200 {
        return SessionState::Unknown;
    }

    let signed_in = body
        .get("session")
        .filter(|session| session.is_object())
        .is_some_and(|session| {
            let id_is_paperclip = session
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| id.starts_with("paperclip:"));
            let has_user = session
                .get("userId")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|user_id| !user_id.is_empty());
            id_is_paperclip && has_user
        });

    if signed_in {
        SessionState::SignedIn
    } else {
        SessionState::Unknown
    }
}

fn detected(
    normalized: &NormalizedRemoteUrl,
    health: &ParsedHealth,
    session_state: SessionState,
    reason: Option<RemotePreflightFailureReason>,
    detail: Option<String>,
    warning: Option<String>,
) -> RemotePreflightResult {
    RemotePreflightResult {
        ok: reason.is_none(),
        normalized_url: normalized.normalized_url.clone(),
        origin: normalized.origin.clone(),
        insecure_transport: normalized.insecure_transport,
        paperclip_detected: true,
        deployment_mode: Some(health.deployment_mode),
        deployment_exposure: health.deployment_exposure,
        auth_ready: health.auth_ready,
        bootstrap_status: health.bootstrap_status,
        bootstrap_invite_active: health.bootstrap_invite_active,
        session_state,
        version: health.version.clone(),
        reason,
        warning,
        detail,
    }
}

fn failure(
    remote_url: &str,
    normalized: Option<&NormalizedRemoteUrl>,
    reason: RemotePreflightFailureReason,
    detail: impl Into<String>,
    warning: Option<String>,
) -> RemotePreflightResult {
    RemotePreflightResult {
        ok: false,
        normalized_url: normalized
            .map(|value| value.normalized_url.clone())
            .unwrap_or_else(|| remote_url.to_string()),
        origin: normalized
            .map(|value| value.origin.clone())
            .unwrap_or_default(),
        insecure_transport: normalized.is_some_and(|value| value.insecure_transport),
        paperclip_detected: false,
        deployment_mode: None,
        deployment_exposure: None,
        auth_ready: None,
        bootstrap_status: None,
        bootstrap_invite_active: None,
        session_state: SessionState::Unknown,
        version: None,
        reason: Some(reason),
        warning,
        detail: Some(detail.into()),
    }
}

fn classify_probe_error(message: &str) -> RemotePreflightFailureReason {
    let lowered = message.to_ascii_lowercase();
    let tls = ["self-signed", "certificate", "unable to verify", "tls"]
        .iter()
        .any(|needle| lowered.contains(needle));

    if tls {
        RemotePreflightFailureReason::TlsError
    } else {
        RemotePreflightFailureReason::Unreachable
    }
}

fn build_version_warning(
    remote_version: Option<&str>,
    local_server_version: Option<&str>,
    existing_warning: Option<&str>,
) -> Option<String> {
    let mut warnings: Vec<String> = existing_warning.map(str::to_string).into_iter().collect();

    if let (Some(remote), Some(local)) = (remote_version, local_server_version) {
        if remote != local {
            warnings.push(format!(
                "Remote Paperclip version {remote} differs from the bundled desktop server \
                 version {local}."
            ));
        }
    }

    (!warnings.is_empty()).then(|| warnings.join(" "))
}

fn join(origin: &str, path: &str) -> Result<Url, String> {
    Url::parse(origin)
        .and_then(|base| base.join(path))
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::http::fake::FakeProbe;
    use serde_json::json;

    const HOST: &str = "https://paperclip-host.tailnet.ts.net";

    fn full_health(mode: &str, auth_ready: bool, bootstrap: &str) -> serde_json::Value {
        json!({
            "status": "ok",
            "version": "2026.403.0",
            "deploymentMode": mode,
            "deploymentExposure": "private",
            "authReady": auth_ready,
            "bootstrapStatus": bootstrap,
            "bootstrapInviteActive": false,
        })
    }

    fn redacted_health(bootstrap: &str, invite_active: bool) -> serde_json::Value {
        json!({
            "status": "ok",
            "deploymentMode": "authenticated",
            "bootstrapStatus": bootstrap,
            "bootstrapInviteActive": invite_active,
        })
    }

    fn auth_required() -> Result<super::super::http::ProbeResponse, super::super::http::ProbeError>
    {
        FakeProbe::json(401, json!({ "error": "Board authentication required" }))
    }

    fn signed_in_session(
    ) -> Result<super::super::http::ProbeResponse, super::super::http::ProbeError> {
        FakeProbe::json(
            200,
            json!({
                "session": { "id": "paperclip:session:1", "userId": "user-1" },
                "user": { "id": "user-1" },
            }),
        )
    }

    async fn run(
        remote_url: &str,
        responses: Vec<Result<super::super::http::ProbeResponse, super::super::http::ProbeError>>,
    ) -> (RemotePreflightResult, FakeProbe) {
        let probe = FakeProbe::new(responses);
        let result = preflight_remote_connection(&probe, PreflightOptions::new(remote_url)).await;
        (result, probe)
    }

    #[tokio::test]
    async fn accepts_authenticated_paperclip_with_active_session() {
        let probe = FakeProbe::new(vec![
            FakeProbe::json(200, full_health("authenticated", true, "ready")),
            signed_in_session(),
        ]);
        let result = preflight_remote_connection(
            &probe,
            PreflightOptions {
                remote_url: HOST,
                local_server_version: Some("2026.403.0"),
                timeout: DEFAULT_TIMEOUT,
            },
        )
        .await;

        assert!(result.ok);
        assert!(!result.insecure_transport);
        assert_eq!(result.session_state, SessionState::SignedIn);
        assert_eq!(result.deployment_mode, Some(DeploymentMode::Authenticated));
        assert_eq!(result.bootstrap_status, Some(BootstrapStatus::Ready));
        assert_eq!(result.warning, None);
    }

    #[tokio::test]
    async fn treats_401_session_probe_as_sign_in_required() {
        let (result, _) = run(
            HOST,
            vec![
                FakeProbe::json(200, full_health("authenticated", true, "bootstrap_pending")),
                auth_required(),
            ],
        )
        .await;

        assert!(result.ok);
        assert_eq!(result.session_state, SessionState::SignedOut);
        assert_eq!(
            result.bootstrap_status,
            Some(BootstrapStatus::BootstrapPending)
        );
    }

    #[tokio::test]
    async fn accepts_redacted_health_with_auth_required_session() {
        let (result, _) = run(
            HOST,
            vec![
                FakeProbe::json(200, redacted_health("ready", false)),
                auth_required(),
            ],
        )
        .await;

        assert!(result.ok);
        assert_eq!(result.session_state, SessionState::SignedOut);
        assert_eq!(result.deployment_mode, Some(DeploymentMode::Authenticated));
        assert_eq!(result.deployment_exposure, None);
        assert_eq!(result.auth_ready, None);
        assert_eq!(result.bootstrap_status, Some(BootstrapStatus::Ready));
        assert_eq!(result.bootstrap_invite_active, Some(false));
    }

    #[tokio::test]
    async fn accepts_redacted_health_with_active_session_and_normalises_the_url() {
        let (result, _) = run(
            "https://paperclip-host.tailnet.ts.net/some/path?ignored=true#hash",
            vec![
                FakeProbe::json(200, redacted_health("bootstrap_pending", true)),
                signed_in_session(),
            ],
        )
        .await;

        assert!(result.ok);
        assert_eq!(
            result.normalized_url,
            "https://paperclip-host.tailnet.ts.net/"
        );
        assert_eq!(result.session_state, SessionState::SignedIn);
        assert_eq!(
            result.bootstrap_status,
            Some(BootstrapStatus::BootstrapPending)
        );
        assert_eq!(result.bootstrap_invite_active, Some(true));
    }

    #[tokio::test]
    async fn rejects_redacted_health_with_a_generic_401() {
        let (result, _) = run(
            "https://example.com",
            vec![
                FakeProbe::json(200, redacted_health("ready", false)),
                FakeProbe::json(401, json!({ "error": "Unauthorized" })),
            ],
        )
        .await;

        assert!(!result.ok);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::NotPaperclip)
        );
        assert!(result.paperclip_detected);
        assert_eq!(result.session_state, SessionState::Unknown);
    }

    #[tokio::test]
    async fn rejects_redacted_health_without_bootstrap_invite_state() {
        let (result, _) = run(
            HOST,
            vec![FakeProbe::json(
                200,
                json!({
                    "status": "ok",
                    "deploymentMode": "authenticated",
                    "bootstrapStatus": "ready",
                }),
            )],
        )
        .await;

        assert!(!result.ok);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::NotPaperclip)
        );
        assert!(!result.paperclip_detected);
    }

    #[tokio::test]
    async fn treats_explicit_auth_ready_false_as_auth_not_ready() {
        let (result, _) = run(
            HOST,
            vec![FakeProbe::json(
                200,
                full_health("authenticated", false, "ready"),
            )],
        )
        .await;

        assert!(!result.ok);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::AuthNotReady)
        );
        assert!(result.paperclip_detected);
    }

    #[tokio::test]
    async fn blocks_local_trusted_remotes() {
        let (result, _) = run(
            "https://192.168.1.50:3100",
            vec![FakeProbe::json(
                200,
                full_health("local_trusted", true, "ready"),
            )],
        )
        .await;

        assert!(!result.ok);
        assert!(!result.insecure_transport);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::UnsupportedLocalTrusted)
        );
    }

    #[tokio::test]
    async fn rejects_unsupported_schemes_before_any_network_call() {
        let (result, probe) = run("ftp://paperclip.example.com", vec![]).await;

        assert!(!result.ok);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::InvalidUrl)
        );
        assert!(result
            .detail
            .unwrap()
            .to_lowercase()
            .contains("http or https"));
        assert!(probe.calls().is_empty(), "no request may be made");
    }

    #[tokio::test]
    async fn allows_http_remotes_and_carries_the_insecurity_warning() {
        let (result, _) = run(
            "http://paperclip.example.com:3200",
            vec![
                FakeProbe::json(200, full_health("authenticated", true, "ready")),
                auth_required(),
            ],
        )
        .await;

        assert!(result.ok);
        assert!(result.insecure_transport);
        assert_eq!(result.origin, "http://paperclip.example.com:3200");
        assert_eq!(result.session_state, SessionState::SignedOut);
        assert!(result.warning.unwrap().contains("without TLS"));
    }

    #[tokio::test]
    async fn rejects_non_paperclip_endpoints() {
        let (result, _) = run(
            "https://example.com",
            vec![FakeProbe::json(200, json!({ "hello": "world" }))],
        )
        .await;

        assert!(!result.ok);
        assert!(!result.insecure_transport);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::NotPaperclip)
        );
    }

    #[tokio::test]
    async fn rejects_health_responses_that_are_not_http_200() {
        let (result, _) = run(
            HOST,
            vec![FakeProbe::json(503, redacted_health("ready", false))],
        )
        .await;

        assert!(!result.ok);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::NotPaperclip)
        );
    }

    /// The probe paths are fixed and same-origin: whatever path, query, or
    /// fragment the user pasted must never influence what gets requested.
    #[tokio::test]
    async fn uses_fixed_same_origin_probe_paths() {
        let (result, probe) = run(
            "https://paperclip-host.tailnet.ts.net/user/input?next=https://evil.test#fragment",
            vec![
                FakeProbe::json(200, redacted_health("ready", false)),
                auth_required(),
            ],
        )
        .await;

        assert!(result.ok);
        assert_eq!(
            probe.calls(),
            vec![
                "https://paperclip-host.tailnet.ts.net/api/health",
                "https://paperclip-host.tailnet.ts.net/api/auth/get-session",
            ]
        );
    }

    #[tokio::test]
    async fn classifies_tls_failures() {
        let (result, _) = run(
            "https://badcert.example.com",
            vec![FakeProbe::failure(
                "error sending request: self-signed certificate",
            )],
        )
        .await;

        assert!(!result.ok);
        assert!(!result.insecure_transport);
        assert_eq!(result.reason, Some(RemotePreflightFailureReason::TlsError));
    }

    #[tokio::test]
    async fn classifies_other_transport_failures_as_unreachable() {
        let (result, _) = run(
            HOST,
            vec![FakeProbe::failure(
                "error sending request: connection refused",
            )],
        )
        .await;

        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::Unreachable)
        );
    }

    #[tokio::test]
    async fn reports_unhealthy_but_recognisable_instances_as_unreachable() {
        let (result, _) = run(
            HOST,
            vec![FakeProbe::json(
                200,
                json!({
                    "status": "starting",
                    "deploymentMode": "authenticated",
                    "deploymentExposure": "private",
                    "authReady": true,
                }),
            )],
        )
        .await;

        assert!(!result.ok);
        assert!(result.paperclip_detected);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::Unreachable)
        );
    }

    #[tokio::test]
    async fn warns_when_the_remote_version_differs_from_the_bundled_one() {
        let probe = FakeProbe::new(vec![
            FakeProbe::json(200, full_health("authenticated", true, "ready")),
            signed_in_session(),
        ]);
        let result = preflight_remote_connection(
            &probe,
            PreflightOptions {
                remote_url: HOST,
                local_server_version: Some("2026.707.0"),
                timeout: DEFAULT_TIMEOUT,
            },
        )
        .await;

        assert!(result.ok);
        let warning = result.warning.unwrap();
        assert!(warning.contains("2026.403.0"));
        assert!(warning.contains("2026.707.0"));
    }

    #[tokio::test]
    async fn a_non_json_session_response_is_unknown() {
        let (result, _) = run(
            HOST,
            vec![
                FakeProbe::json(200, redacted_health("ready", false)),
                FakeProbe::non_json(200),
            ],
        )
        .await;

        assert!(!result.ok);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::NotPaperclip)
        );
    }

    /// A session id that does not carry Paperclip's own prefix must not be
    /// accepted as a signed-in session.
    #[tokio::test]
    async fn a_foreign_session_payload_is_unknown() {
        let (result, _) = run(
            HOST,
            vec![
                FakeProbe::json(200, redacted_health("ready", false)),
                FakeProbe::json(
                    200,
                    json!({ "session": { "id": "other:1", "userId": "user-1" } }),
                ),
            ],
        )
        .await;

        assert!(!result.ok);
        assert_eq!(
            result.reason,
            Some(RemotePreflightFailureReason::NotPaperclip)
        );
    }
}
