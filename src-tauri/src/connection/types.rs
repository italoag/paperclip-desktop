//! Shared connection vocabulary. Port of `src/connection/types.ts`.
//!
//! The serde representations are load-bearing: `connections.json` written by the
//! Electron build must keep deserialising, so every field stays camelCase and
//! optional fields keep skipping when empty.

use serde::{Deserialize, Serialize};

pub const LOCAL_PROFILE_ID: &str = "local_embedded";
pub const CONNECTIONS_FILE_NAME: &str = "connections.json";
pub const CONNECTIONS_FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    LocalEmbedded,
    RemoteExisting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionHealth {
    Healthy,
    AuthRequired,
    Unsupported,
    Unreachable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    LocalTrusted,
    Authenticated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentExposure {
    Private,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    SignedIn,
    SignedOut,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapStatus {
    Ready,
    BootstrapPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemotePreflightFailureReason {
    InvalidUrl,
    Unreachable,
    TlsError,
    NotPaperclip,
    UnsupportedLocalTrusted,
    IncompatibleVersion,
    AuthNotReady,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub mode: ConnectionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_insecure_http: Option<bool>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_health: Option<ConnectionHealth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_deployment_mode: Option<DeploymentMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_session_state: Option<SessionState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionState {
    pub active_profile_id: Option<String>,
    pub always_show_chooser: bool,
    pub auto_connect_last_profile: bool,
    pub chooser_mode: ConnectionMode,
    pub local_profile_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_last_connected_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_last_health: Option<ConnectionHealth>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            active_profile_id: None,
            always_show_chooser: true,
            auto_connect_last_profile: false,
            chooser_mode: ConnectionMode::LocalEmbedded,
            local_profile_name: "Local".to_string(),
            local_last_connected_at: None,
            local_last_health: Some(ConnectionHealth::Healthy),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedConnectionsFile {
    pub version: u32,
    pub state: ConnectionState,
    pub remote_profiles: Vec<ConnectionProfile>,
}

impl Default for PersistedConnectionsFile {
    fn default() -> Self {
        Self {
            version: CONNECTIONS_FILE_VERSION,
            state: ConnectionState::default(),
            remote_profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePreflightResult {
    pub ok: bool,
    pub normalized_url: String,
    pub origin: String,
    pub insecure_transport: bool,
    pub paperclip_detected: bool,
    pub deployment_mode: Option<DeploymentMode>,
    pub deployment_exposure: Option<DeploymentExposure>,
    pub auth_ready: Option<bool>,
    pub bootstrap_status: Option<BootstrapStatus>,
    pub bootstrap_invite_active: Option<bool>,
    pub session_state: SessionState,
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<RemotePreflightFailureReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The launcher UI and the persisted file both use the TypeScript spellings.
    #[test]
    fn enums_serialise_with_the_legacy_wire_values() {
        let pairs = [
            (
                serde_json::to_string(&ConnectionMode::LocalEmbedded).unwrap(),
                "\"local_embedded\"",
            ),
            (
                serde_json::to_string(&ConnectionMode::RemoteExisting).unwrap(),
                "\"remote_existing\"",
            ),
            (
                serde_json::to_string(&ConnectionHealth::AuthRequired).unwrap(),
                "\"auth_required\"",
            ),
            (
                serde_json::to_string(&DeploymentMode::LocalTrusted).unwrap(),
                "\"local_trusted\"",
            ),
            (
                serde_json::to_string(&SessionState::SignedIn).unwrap(),
                "\"signed_in\"",
            ),
            (
                serde_json::to_string(&BootstrapStatus::BootstrapPending).unwrap(),
                "\"bootstrap_pending\"",
            ),
            (
                serde_json::to_string(&RemotePreflightFailureReason::UnsupportedLocalTrusted)
                    .unwrap(),
                "\"unsupported_local_trusted\"",
            ),
        ];

        for (actual, expected) in pairs {
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn profiles_round_trip_through_the_legacy_camel_case_shape() {
        let json = serde_json::json!({
            "id": "abc",
            "name": "Home",
            "mode": "remote_existing",
            "remoteUrl": "https://example.com/",
            "allowInsecureHttp": true,
            "createdAt": "2026-04-06T10:00:00.000Z",
            "updatedAt": "2026-04-06T10:00:00.000Z",
            "lastConnectedAt": "2026-04-06T11:00:00.000Z",
            "lastHealth": "healthy",
            "lastDeploymentMode": "authenticated",
            "lastSessionState": "signed_in"
        });

        let profile: ConnectionProfile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(profile.remote_url.as_deref(), Some("https://example.com/"));
        assert_eq!(serde_json::to_value(&profile).unwrap(), json);
    }

    /// Optional fields must disappear rather than serialise as null, matching
    /// what the Electron build wrote.
    #[test]
    fn absent_optional_fields_are_omitted() {
        let profile = ConnectionProfile {
            id: "local_embedded".into(),
            name: "Local".into(),
            mode: ConnectionMode::LocalEmbedded,
            remote_url: None,
            allow_insecure_http: None,
            created_at: "1970-01-01T00:00:00.000Z".into(),
            updated_at: "1970-01-01T00:00:00.000Z".into(),
            last_connected_at: None,
            last_health: Some(ConnectionHealth::Healthy),
            last_deployment_mode: None,
            last_session_state: None,
        };

        let value = serde_json::to_value(&profile).unwrap();
        let object = value.as_object().unwrap();
        assert!(!object.contains_key("remoteUrl"));
        assert!(!object.contains_key("allowInsecureHttp"));
        assert!(!object.contains_key("lastConnectedAt"));
        assert_eq!(object["lastHealth"], "healthy");
    }
}
