//! Persistence for saved connections. Port of `src/connection/profiles.ts`.
//!
//! # Compatibility contract
//!
//! `connections.json` is written by the Electron build that users already have
//! installed, so this module must keep reading those files byte-for-byte. That
//! is why parsing goes through `serde_json::Value` and an explicit sanitising
//! pass rather than a plain `Deserialize`: the original silently repairs partial
//! state and *drops* individual malformed profiles instead of failing the whole
//! file, and losing that behaviour would mean losing a user's saved remotes on
//! the first slightly-off record.

use std::path::{Path, PathBuf};

use serde_json::Value;
use uuid::Uuid;

use super::types::{
    ConnectionHealth, ConnectionMode, ConnectionProfile, ConnectionState, DeploymentMode,
    PersistedConnectionsFile, RemotePreflightResult, SessionState, CONNECTIONS_FILE_NAME,
    CONNECTIONS_FILE_VERSION, LOCAL_PROFILE_ID,
};
use super::validate::{normalize_remote_url, RemoteUrlError};

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error(transparent)]
    Url(#[from] RemoteUrlError),

    #[error("HTTP remotes require confirming that you want to allow an insecure connection.")]
    InsecureNotAcknowledged,

    #[error("Unknown remote profile: {0}")]
    UnknownProfile(String),

    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn connections_file_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(CONNECTIONS_FILE_NAME)
}

/// Exactly the shape `Date.prototype.toISOString()` produced, down to always
/// emitting three subsecond digits.
///
/// This is not cosmetic. Timestamps are compared as **strings** (both here in
/// `recent_remote_profiles` and previously via `localeCompare`), and RFC 3339
/// permits dropping a zero subsecond. `...T00:00:00Z` would then sort *after*
/// `...T00:00:00.500Z`, because `'Z' > '.'`, silently reordering recents.
const ISO8601_MS: &[time::format_description::FormatItem<'static>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
);

pub fn now_iso8601() -> String {
    time::OffsetDateTime::now_utc()
        .format(ISO8601_MS)
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".to_string())
}

pub struct SaveRemoteProfile<'a> {
    pub profile_id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub remote_url: &'a str,
    pub allow_insecure_http: bool,
    pub now: Option<String>,
}

impl<'a> SaveRemoteProfile<'a> {
    pub fn new(remote_url: &'a str) -> Self {
        Self {
            profile_id: None,
            name: None,
            remote_url,
            allow_insecure_http: false,
            now: None,
        }
    }
}

pub struct ConnectionStore {
    file_path: PathBuf,
    cache: PersistedConnectionsFile,
}

impl ConnectionStore {
    pub fn open(file_path: impl Into<PathBuf>) -> Self {
        let file_path = file_path.into();
        let cache = std::fs::read_to_string(&file_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .map(|value| sanitize_file(&value))
            .unwrap_or_default();

        Self { file_path, cache }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn snapshot(&self) -> PersistedConnectionsFile {
        self.cache.clone()
    }

    /// The synthetic local profile always leads the list; it has no persisted
    /// record of its own, only the fields tracked in `ConnectionState`.
    pub fn list_profiles(&self) -> Vec<ConnectionProfile> {
        let mut profiles = vec![local_profile(&self.cache.state)];
        profiles.extend(self.cache.remote_profiles.iter().cloned());
        profiles
    }

    pub fn get_profile(&self, profile_id: &str) -> Option<ConnectionProfile> {
        if profile_id == LOCAL_PROFILE_ID {
            return Some(local_profile(&self.cache.state));
        }
        self.find(profile_id).cloned()
    }

    /// Which profile, if any, should be connected without showing the chooser.
    ///
    /// An HTTP remote that was never explicitly acknowledged is refused here as
    /// well as at save time, so a hand-edited file cannot silently auto-connect
    /// over plaintext.
    pub fn startup_profile_id(&self) -> Option<String> {
        let state = &self.cache.state;
        if state.always_show_chooser || !state.auto_connect_last_profile {
            return None;
        }

        let active = state.active_profile_id.as_deref()?;
        if active == LOCAL_PROFILE_ID {
            return Some(LOCAL_PROFILE_ID.to_string());
        }

        self.cache
            .remote_profiles
            .iter()
            .find(|profile| {
                profile.id == active
                    && (!profile
                        .remote_url
                        .as_deref()
                        .is_some_and(|url| url.starts_with("http://"))
                        || profile.allow_insecure_http == Some(true))
            })
            .map(|profile| profile.id.clone())
    }

    pub fn save_remote_profile(
        &mut self,
        input: SaveRemoteProfile<'_>,
    ) -> Result<ConnectionProfile, ProfileError> {
        let now = input.now.unwrap_or_else(now_iso8601);
        let normalized = normalize_remote_url(input.remote_url)?;
        if normalized.insecure_transport && !input.allow_insecure_http {
            return Err(ProfileError::InsecureNotAcknowledged);
        }

        let name = sanitize_profile_name(input.name, &normalized.origin);
        let allow_insecure = normalized.insecure_transport.then_some(true);

        let existing = input
            .profile_id
            .and_then(|id| self.cache.remote_profiles.iter().position(|p| p.id == id));

        let profile = match existing {
            Some(index) => {
                let profile = &mut self.cache.remote_profiles[index];
                profile.name = name;
                profile.remote_url = Some(normalized.normalized_url);
                profile.allow_insecure_http = allow_insecure;
                profile.updated_at = now;
                profile.clone()
            }
            None => {
                let profile = ConnectionProfile {
                    id: Uuid::new_v4().to_string(),
                    name,
                    mode: ConnectionMode::RemoteExisting,
                    remote_url: Some(normalized.normalized_url),
                    allow_insecure_http: allow_insecure,
                    created_at: now.clone(),
                    updated_at: now,
                    last_connected_at: None,
                    last_health: Some(ConnectionHealth::Unknown),
                    last_deployment_mode: None,
                    last_session_state: Some(SessionState::Unknown),
                };
                self.cache.remote_profiles.push(profile.clone());
                profile
            }
        };

        self.persist()?;
        Ok(profile)
    }

    pub fn duplicate_remote_profile(
        &mut self,
        profile_id: &str,
        now: Option<String>,
    ) -> Result<ConnectionProfile, ProfileError> {
        let now = now.unwrap_or_else(now_iso8601);
        let existing = self.require(profile_id)?.clone();

        let duplicate = ConnectionProfile {
            id: Uuid::new_v4().to_string(),
            name: format!("{} (copy)", existing.name),
            created_at: now.clone(),
            updated_at: now,
            last_connected_at: None,
            ..existing
        };

        self.cache.remote_profiles.push(duplicate.clone());
        self.persist()?;
        Ok(duplicate)
    }

    pub fn delete_remote_profile(&mut self, profile_id: &str) -> Result<(), ProfileError> {
        let before = self.cache.remote_profiles.len();
        self.cache
            .remote_profiles
            .retain(|profile| profile.id != profile_id);
        if before == self.cache.remote_profiles.len() {
            return Ok(());
        }

        if self.cache.state.active_profile_id.as_deref() == Some(profile_id) {
            self.cache.state.active_profile_id = None;
        }
        self.persist()
    }

    pub fn set_always_show_chooser(&mut self, value: bool) -> Result<(), ProfileError> {
        self.cache.state.always_show_chooser = value;
        if value {
            self.cache.state.auto_connect_last_profile = false;
        }
        self.persist()
    }

    pub fn set_chooser_mode(&mut self, mode: ConnectionMode) -> Result<(), ProfileError> {
        self.cache.state.chooser_mode = mode;
        self.persist()
    }

    pub fn set_remembered_profile(
        &mut self,
        profile_id: Option<&str>,
        remember: bool,
    ) -> Result<(), ProfileError> {
        self.cache.state.active_profile_id = profile_id.map(str::to_string);
        self.cache.state.auto_connect_last_profile = remember;
        self.cache.state.always_show_chooser = !remember;
        self.persist()
    }

    pub fn record_connection_result(
        &mut self,
        profile_id: &str,
        result: Option<&RemotePreflightResult>,
        now: Option<String>,
    ) -> Result<(), ProfileError> {
        let now = now.unwrap_or_else(now_iso8601);

        if profile_id == LOCAL_PROFILE_ID {
            self.cache.state.active_profile_id = Some(LOCAL_PROFILE_ID.to_string());
            self.cache.state.local_last_connected_at = Some(now);
            self.cache.state.local_last_health = Some(ConnectionHealth::Healthy);
            return self.persist();
        }

        let profile = self.require_mut(profile_id)?;
        profile.last_connected_at = Some(now.clone());
        profile.updated_at = now;
        if let Some(result) = result {
            apply_result(profile, result);
        }
        self.cache.state.active_profile_id = Some(profile_id.to_string());
        self.persist()
    }

    pub fn record_remote_health(
        &mut self,
        profile_id: &str,
        result: &RemotePreflightResult,
        now: Option<String>,
    ) -> Result<(), ProfileError> {
        let now = now.unwrap_or_else(now_iso8601);
        let profile = self.require_mut(profile_id)?;
        profile.updated_at = now;
        apply_result(profile, result);
        self.persist()
    }

    pub fn sync_remote_profile_url(
        &mut self,
        profile_id: &str,
        normalized_url: &str,
        allow_insecure_http: bool,
        now: Option<String>,
    ) -> Result<(), ProfileError> {
        let now = now.unwrap_or_else(now_iso8601);
        let profile = self.require_mut(profile_id)?;
        profile.remote_url = Some(normalized_url.to_string());
        profile.allow_insecure_http = allow_insecure_http.then_some(true);
        profile.updated_at = now;
        self.persist()
    }

    /// Most recently used remotes first, for the Connection menu.
    pub fn recent_remote_profiles(&self, limit: usize) -> Vec<ConnectionProfile> {
        let mut profiles = self.cache.remote_profiles.clone();
        profiles.sort_by(|left, right| {
            let key = |profile: &ConnectionProfile| {
                profile
                    .last_connected_at
                    .clone()
                    .unwrap_or_else(|| profile.updated_at.clone())
            };
            key(right).cmp(&key(left))
        });
        profiles.truncate(limit);
        profiles
    }

    fn find(&self, profile_id: &str) -> Option<&ConnectionProfile> {
        self.cache
            .remote_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
    }

    fn require(&self, profile_id: &str) -> Result<&ConnectionProfile, ProfileError> {
        self.find(profile_id)
            .ok_or_else(|| ProfileError::UnknownProfile(profile_id.to_string()))
    }

    fn require_mut(&mut self, profile_id: &str) -> Result<&mut ConnectionProfile, ProfileError> {
        self.cache
            .remote_profiles
            .iter_mut()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| ProfileError::UnknownProfile(profile_id.to_string()))
    }

    fn persist(&mut self) -> Result<(), ProfileError> {
        // Re-sanitising on write matches the original and keeps a bad in-memory
        // mutation from ever reaching disk.
        let value = serde_json::to_value(&self.cache).unwrap_or(Value::Null);
        self.cache = sanitize_file(&value);

        let write = || -> Result<(), std::io::Error> {
            if let Some(parent) = self.file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let serialized = serde_json::to_string_pretty(&self.cache)?;
            std::fs::write(&self.file_path, serialized)
        };

        write().map_err(|source| ProfileError::Write {
            path: self.file_path.clone(),
            source,
        })
    }
}

fn apply_result(profile: &mut ConnectionProfile, result: &RemotePreflightResult) {
    profile.remote_url = Some(result.normalized_url.clone());
    profile.allow_insecure_http = result.insecure_transport.then_some(true);
    profile.last_health = Some(derive_health(result));
    profile.last_deployment_mode = result.deployment_mode;
    profile.last_session_state = Some(result.session_state);
}

fn derive_health(result: &RemotePreflightResult) -> ConnectionHealth {
    use super::types::RemotePreflightFailureReason as Reason;

    if result.ok && result.session_state == SessionState::SignedIn {
        return ConnectionHealth::Healthy;
    }
    if result.ok && result.session_state == SessionState::SignedOut {
        return ConnectionHealth::AuthRequired;
    }
    match result.reason {
        Some(Reason::UnsupportedLocalTrusted) | Some(Reason::AuthNotReady) => {
            ConnectionHealth::Unsupported
        }
        Some(Reason::TlsError) | Some(Reason::Unreachable) => ConnectionHealth::Unreachable,
        _ => ConnectionHealth::Unknown,
    }
}

fn local_profile(state: &ConnectionState) -> ConnectionProfile {
    const EPOCH: &str = "1970-01-01T00:00:00.000Z";

    ConnectionProfile {
        id: LOCAL_PROFILE_ID.to_string(),
        name: state.local_profile_name.clone(),
        mode: ConnectionMode::LocalEmbedded,
        remote_url: None,
        allow_insecure_http: None,
        created_at: EPOCH.to_string(),
        updated_at: state
            .local_last_connected_at
            .clone()
            .unwrap_or_else(|| EPOCH.to_string()),
        last_connected_at: state.local_last_connected_at.clone(),
        last_health: Some(state.local_last_health.unwrap_or(ConnectionHealth::Healthy)),
        last_deployment_mode: None,
        last_session_state: None,
    }
}

// ---------------------------------------------------------------------------
// Lenient parsing — mirrors sanitize* in profiles.ts
// ---------------------------------------------------------------------------

fn sanitize_file(raw: &Value) -> PersistedConnectionsFile {
    if !raw.is_object() {
        return PersistedConnectionsFile::default();
    }

    PersistedConnectionsFile {
        version: CONNECTIONS_FILE_VERSION,
        state: sanitize_state(raw.get("state")),
        remote_profiles: raw
            .get("remoteProfiles")
            .and_then(Value::as_array)
            .map(|profiles| {
                profiles
                    .iter()
                    .filter_map(sanitize_remote_profile)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn sanitize_state(raw: Option<&Value>) -> ConnectionState {
    let Some(raw) = raw.filter(|value| value.is_object()) else {
        return ConnectionState::default();
    };

    let string = |key: &str| raw.get(key).and_then(Value::as_str);

    ConnectionState {
        active_profile_id: string("activeProfileId").map(str::to_string),
        // Defaults to true: only an explicit `false` turns the chooser off.
        always_show_chooser: raw.get("alwaysShowChooser") != Some(&Value::Bool(false)),
        auto_connect_last_profile: raw.get("autoConnectLastProfile") == Some(&Value::Bool(true)),
        chooser_mode: match string("chooserMode") {
            Some("remote_existing") => ConnectionMode::RemoteExisting,
            _ => ConnectionMode::LocalEmbedded,
        },
        local_profile_name: string("localProfileName")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("Local")
            .to_string(),
        local_last_connected_at: string("localLastConnectedAt").map(str::to_string),
        local_last_health: Some(
            sanitize_health(string("localLastHealth")).unwrap_or(ConnectionHealth::Healthy),
        ),
    }
}

fn sanitize_remote_profile(raw: &Value) -> Option<ConnectionProfile> {
    if !raw.is_object() || raw.get("mode").and_then(Value::as_str) != Some("remote_existing") {
        return None;
    }

    let string = |key: &str| raw.get(key).and_then(Value::as_str);

    let id = string("id")?;
    let remote_url = string("remoteUrl")?;
    // A profile whose URL no longer validates is dropped rather than repaired.
    let normalized = normalize_remote_url(remote_url).ok()?;

    let created_at = string("createdAt")
        .map(str::to_string)
        .unwrap_or_else(now_iso8601);
    let updated_at = string("updatedAt")
        .map(str::to_string)
        .unwrap_or_else(|| created_at.clone());

    Some(ConnectionProfile {
        id: id.to_string(),
        name: sanitize_profile_name(string("name"), &normalized.origin),
        mode: ConnectionMode::RemoteExisting,
        remote_url: Some(normalized.normalized_url),
        allow_insecure_http: (normalized.insecure_transport
            && raw.get("allowInsecureHttp") == Some(&Value::Bool(true)))
        .then_some(true),
        created_at,
        updated_at,
        last_connected_at: string("lastConnectedAt").map(str::to_string),
        last_health: Some(
            sanitize_health(string("lastHealth")).unwrap_or(ConnectionHealth::Unknown),
        ),
        last_deployment_mode: match string("lastDeploymentMode") {
            Some("authenticated") => Some(DeploymentMode::Authenticated),
            Some("local_trusted") => Some(DeploymentMode::LocalTrusted),
            _ => None,
        },
        last_session_state: Some(match string("lastSessionState") {
            Some("signed_in") => SessionState::SignedIn,
            Some("signed_out") => SessionState::SignedOut,
            _ => SessionState::Unknown,
        }),
    })
}

fn sanitize_profile_name(name: Option<&str>, origin: &str) -> String {
    if let Some(trimmed) = name.map(str::trim).filter(|name| !name.is_empty()) {
        return trimmed.to_string();
    }

    url::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
}

fn sanitize_health(value: Option<&str>) -> Option<ConnectionHealth> {
    match value {
        Some("healthy") => Some(ConnectionHealth::Healthy),
        Some("auth_required") => Some(ConnectionHealth::AuthRequired),
        Some("unsupported") => Some(ConnectionHealth::Unsupported),
        Some("unreachable") => Some(ConnectionHealth::Unreachable),
        Some("unknown") => Some(ConnectionHealth::Unknown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::types::{BootstrapStatus, DeploymentExposure};
    use time::format_description::well_known::Rfc3339;

    fn temp_store() -> (tempfile::TempDir, ConnectionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(connections_file_path(dir.path()));
        (dir, store)
    }

    fn signed_out_result() -> RemotePreflightResult {
        RemotePreflightResult {
            ok: true,
            normalized_url: "https://paperclip-host.tailnet.ts.net/".into(),
            origin: "https://paperclip-host.tailnet.ts.net".into(),
            insecure_transport: false,
            paperclip_detected: true,
            deployment_mode: Some(DeploymentMode::Authenticated),
            deployment_exposure: Some(DeploymentExposure::Private),
            auth_ready: Some(true),
            bootstrap_status: Some(BootstrapStatus::Ready),
            bootstrap_invite_active: Some(false),
            session_state: SessionState::SignedOut,
            version: Some("2026.403.0".into()),
            reason: None,
            warning: None,
            detail: None,
        }
    }

    #[test]
    fn persists_remote_profiles_and_startup_preference() {
        let (dir, mut store) = temp_store();

        let profile = store
            .save_remote_profile(SaveRemoteProfile {
                name: Some("Home Server"),
                now: Some("2026-04-06T10:00:00.000Z".into()),
                ..SaveRemoteProfile::new("https://paperclip-host.tailnet.ts.net/dashboard")
            })
            .unwrap();
        store
            .set_remembered_profile(Some(&profile.id), true)
            .unwrap();
        store
            .record_connection_result(&profile.id, Some(&signed_out_result()), None)
            .unwrap();

        let reloaded = ConnectionStore::open(connections_file_path(dir.path()));
        let snapshot = reloaded.snapshot();

        assert_eq!(snapshot.remote_profiles.len(), 1);
        assert_eq!(
            snapshot.remote_profiles[0].remote_url.as_deref(),
            Some("https://paperclip-host.tailnet.ts.net/")
        );
        assert_eq!(
            snapshot.remote_profiles[0].last_health,
            Some(ConnectionHealth::AuthRequired)
        );
        assert_eq!(reloaded.startup_profile_id(), Some(profile.id));
    }

    #[test]
    fn keeps_a_synthetic_local_profile() {
        let (_dir, mut store) = temp_store();

        store
            .record_connection_result(
                LOCAL_PROFILE_ID,
                None,
                Some("2026-04-06T10:00:00.000Z".into()),
            )
            .unwrap();

        let local = store.get_profile(LOCAL_PROFILE_ID).unwrap();
        assert_eq!(local.mode, ConnectionMode::LocalEmbedded);
        assert_eq!(
            local.last_connected_at.as_deref(),
            Some("2026-04-06T10:00:00.000Z")
        );
    }

    #[test]
    fn duplicates_and_deletes_remote_profiles() {
        let (_dir, mut store) = temp_store();

        let profile = store
            .save_remote_profile(SaveRemoteProfile {
                name: Some("Dev VM"),
                ..SaveRemoteProfile::new("https://dev.paperclip.internal")
            })
            .unwrap();
        let duplicate = store
            .duplicate_remote_profile(&profile.id, Some("2026-04-06T10:00:00.000Z".into()))
            .unwrap();
        store.delete_remote_profile(&profile.id).unwrap();

        let remotes: Vec<_> = store
            .list_profiles()
            .into_iter()
            .filter(|p| p.mode == ConnectionMode::RemoteExisting)
            .collect();

        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].id, duplicate.id);
        assert!(remotes[0].name.ends_with("(copy)"));
    }

    #[test]
    fn requires_explicit_acknowledgement_before_saving_an_http_profile() {
        let (dir, mut store) = temp_store();

        let refused = store.save_remote_profile(SaveRemoteProfile {
            name: Some("Insecure"),
            ..SaveRemoteProfile::new("http://paperclip.example.com")
        });
        assert!(matches!(
            refused,
            Err(ProfileError::InsecureNotAcknowledged)
        ));

        let profile = store
            .save_remote_profile(SaveRemoteProfile {
                name: Some("Insecure"),
                allow_insecure_http: true,
                ..SaveRemoteProfile::new("http://paperclip.example.com")
            })
            .unwrap();
        assert_eq!(profile.allow_insecure_http, Some(true));

        let reloaded = ConnectionStore::open(connections_file_path(dir.path()));
        assert_eq!(
            reloaded
                .get_profile(&profile.id)
                .unwrap()
                .allow_insecure_http,
            Some(true)
        );
    }

    /// A hand-edited file must not be able to auto-connect over plaintext.
    #[test]
    fn unacknowledged_http_profiles_never_auto_connect() {
        let dir = tempfile::tempdir().unwrap();
        let path = connections_file_path(dir.path());
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "state": {
                    "activeProfileId": "p1",
                    "alwaysShowChooser": false,
                    "autoConnectLastProfile": true,
                    "chooserMode": "remote_existing",
                    "localProfileName": "Local"
                },
                "remoteProfiles": [{
                    "id": "p1",
                    "name": "Sneaky",
                    "mode": "remote_existing",
                    "remoteUrl": "http://evil.example.com/",
                    "createdAt": "2026-04-06T10:00:00.000Z",
                    "updatedAt": "2026-04-06T10:00:00.000Z"
                }]
            })
            .to_string(),
        )
        .unwrap();

        let store = ConnectionStore::open(&path);
        assert_eq!(store.startup_profile_id(), None);
    }

    /// The Electron build repaired partial files instead of failing; losing that
    /// would drop a user's saved remotes.
    #[test]
    fn malformed_records_are_dropped_without_losing_the_valid_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = connections_file_path(dir.path());
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "state": { "chooserMode": "nonsense" },
                "remoteProfiles": [
                    { "id": "good", "name": "Good", "mode": "remote_existing",
                      "remoteUrl": "https://good.example.com/",
                      "createdAt": "2026-01-01T00:00:00.000Z",
                      "updatedAt": "2026-01-01T00:00:00.000Z" },
                    { "id": "no-url", "mode": "remote_existing" },
                    { "id": "bad-scheme", "mode": "remote_existing", "remoteUrl": "ftp://x.test" },
                    { "id": "local-ish", "mode": "local_embedded", "remoteUrl": "https://x.test" },
                    "not an object"
                ]
            })
            .to_string(),
        )
        .unwrap();

        let store = ConnectionStore::open(&path);
        let snapshot = store.snapshot();

        assert_eq!(snapshot.remote_profiles.len(), 1);
        assert_eq!(snapshot.remote_profiles[0].id, "good");
        // Unrecognised values fall back to the documented defaults.
        assert_eq!(snapshot.state.chooser_mode, ConnectionMode::LocalEmbedded);
        assert!(snapshot.state.always_show_chooser);
        assert_eq!(snapshot.state.local_profile_name, "Local");
    }

    #[test]
    fn a_missing_or_corrupt_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let missing = ConnectionStore::open(connections_file_path(dir.path()));
        assert_eq!(missing.snapshot(), PersistedConnectionsFile::default());

        let path = dir.path().join("corrupt.json");
        std::fs::write(&path, "{ not json").unwrap();
        let corrupt = ConnectionStore::open(&path);
        assert_eq!(corrupt.snapshot(), PersistedConnectionsFile::default());
    }

    #[test]
    fn profile_names_fall_back_to_the_host() {
        let (_dir, mut store) = temp_store();
        let profile = store
            .save_remote_profile(SaveRemoteProfile {
                name: Some("   "),
                ..SaveRemoteProfile::new("https://paperclip.example.com:8443/x")
            })
            .unwrap();

        assert_eq!(profile.name, "paperclip.example.com");
    }

    #[test]
    fn recent_profiles_are_most_recently_used_first() {
        let (_dir, mut store) = temp_store();
        for (name, host, when) in [
            (
                "Oldest",
                "https://a.example.com",
                "2026-01-01T00:00:00.000Z",
            ),
            (
                "Newest",
                "https://b.example.com",
                "2026-06-01T00:00:00.000Z",
            ),
            (
                "Middle",
                "https://c.example.com",
                "2026-03-01T00:00:00.000Z",
            ),
        ] {
            let profile = store
                .save_remote_profile(SaveRemoteProfile {
                    name: Some(name),
                    now: Some(when.into()),
                    ..SaveRemoteProfile::new(host)
                })
                .unwrap();
            store
                .record_connection_result(&profile.id, None, Some(when.into()))
                .unwrap();
        }

        let names: Vec<_> = store
            .recent_remote_profiles(2)
            .into_iter()
            .map(|profile| profile.name)
            .collect();
        assert_eq!(names, vec!["Newest", "Middle"]);
    }

    #[test]
    fn remembering_a_profile_disables_the_chooser_and_forgetting_restores_it() {
        let (_dir, mut store) = temp_store();

        store.set_remembered_profile(Some("p1"), true).unwrap();
        assert!(!store.snapshot().state.always_show_chooser);
        assert!(store.snapshot().state.auto_connect_last_profile);

        store.set_remembered_profile(None, false).unwrap();
        assert!(store.snapshot().state.always_show_chooser);
        assert!(!store.snapshot().state.auto_connect_last_profile);

        // Turning the chooser back on must also clear auto-connect.
        store.set_remembered_profile(Some("p1"), true).unwrap();
        store.set_always_show_chooser(true).unwrap();
        assert!(!store.snapshot().state.auto_connect_last_profile);
    }

    #[test]
    fn deleting_the_active_profile_clears_the_active_pointer() {
        let (_dir, mut store) = temp_store();
        let profile = store
            .save_remote_profile(SaveRemoteProfile::new("https://a.example.com"))
            .unwrap();
        store
            .set_remembered_profile(Some(&profile.id), true)
            .unwrap();

        store.delete_remote_profile(&profile.id).unwrap();
        assert_eq!(store.snapshot().state.active_profile_id, None);
    }

    #[test]
    fn unknown_profiles_are_reported_rather_than_panicking() {
        let (_dir, mut store) = temp_store();
        assert!(matches!(
            store.record_connection_result("nope", None, None),
            Err(ProfileError::UnknownProfile(_))
        ));
    }

    #[test]
    fn generated_timestamps_match_the_javascript_shape_exactly() {
        let now = now_iso8601();

        assert!(
            time::OffsetDateTime::parse(&now, &Rfc3339).is_ok(),
            "not RFC3339: {now}"
        );
        // `2026-04-06T10:00:00.000Z` — 24 chars, always three subsecond digits,
        // so string comparison stays a valid chronological ordering.
        assert_eq!(now.len(), 24, "unexpected shape: {now}");
        assert_eq!(&now[19..20], ".", "missing subseconds: {now}");
        assert!(now.ends_with('Z'), "not UTC: {now}");
        assert!(now.as_str() > "2020-01-01T00:00:00.000Z");
    }

    /// Guards the ordering trap the format above exists to avoid.
    #[test]
    fn whole_second_timestamps_still_sort_chronologically() {
        let (_dir, mut store) = temp_store();
        for (name, host, when) in [
            (
                "Earlier",
                "https://a.example.com",
                "2026-01-01T00:00:00.000Z",
            ),
            ("Later", "https://b.example.com", "2026-01-01T00:00:00.500Z"),
        ] {
            let profile = store
                .save_remote_profile(SaveRemoteProfile {
                    name: Some(name),
                    now: Some(when.into()),
                    ..SaveRemoteProfile::new(host)
                })
                .unwrap();
            store
                .record_connection_result(&profile.id, None, Some(when.into()))
                .unwrap();
        }

        let names: Vec<_> = store
            .recent_remote_profiles(2)
            .into_iter()
            .map(|profile| profile.name)
            .collect();
        assert_eq!(names, vec!["Later", "Earlier"]);
    }
}
