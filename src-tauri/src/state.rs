//! Process-wide application state.
//!
//! Replaces the module-level `let` bindings that `src/main.ts` kept at the top
//! of the file (`serverProcess`, `mainWindow`, `currentConnection`,
//! `bootSequence`, `isQuitting`, …). Collecting them behind one managed struct
//! is what lets the boot flows be re-entrant without the ad-hoc guards the
//! original needed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use serde::Serialize;

use crate::connection::profiles::ConnectionStore;
use crate::connection::types::{
    ConnectionMode, ConnectionProfile, ConnectionState, LOCAL_PROFILE_ID,
};
use crate::server::supervisor::RunningServer;

/// Views the launcher can display. Serialised as the kebab-case names the UI
/// already switches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherView {
    Chooser,
    RemoteForm,
    Saved,
    LocalBoot,
    Connecting,
    Error,
}

/// Whether the launcher is a standalone window or a sheet over the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherPresentation {
    Standalone,
    Attached,
}

#[derive(Debug, Clone)]
pub struct CurrentConnection {
    pub mode: ConnectionMode,
    pub profile_id: Option<String>,
    pub start_url: String,
    pub allowed_origin: String,
    pub data_store_id: [u8; 16],
}

/// Filesystem locations resolved once at startup.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub user_data: PathBuf,
    pub app_root: PathBuf,
    pub paperclip_home: PathBuf,
    pub packaged: bool,
}

pub struct AppState {
    pub paths: AppPaths,
    pub store: Mutex<ConnectionStore>,
    /// Async mutex: shutting the server down is an `await`.
    pub server: tokio::sync::Mutex<Option<RunningServer>>,
    pub connection: RwLock<Option<CurrentConnection>>,
    pub launcher_view: Mutex<LauncherView>,
    pub launcher_presentation: Mutex<LauncherPresentation>,
    /// Current zoom factor. Electron's `zoomIn`/`zoomOut` roles tracked this
    /// internally; Tauri only offers an absolute `set_zoom`, so we keep it.
    pub zoom: Mutex<f64>,
    /// Monotonic boot counter.
    ///
    /// Every boot captures the value at entry and re-checks it at each await
    /// point; a mismatch means a newer boot superseded this one and the older
    /// one must unwind instead of installing its window. Same technique as
    /// `bootSequence` in `main.ts`, but the checks cannot be forgotten because
    /// they are funnelled through [`BootToken`].
    boot_sequence: AtomicU64,
    pub is_quitting: AtomicBool,
}

impl AppState {
    pub fn new(paths: AppPaths, store: ConnectionStore) -> Self {
        Self {
            paths,
            store: Mutex::new(store),
            server: tokio::sync::Mutex::new(None),
            connection: RwLock::new(None),
            launcher_view: Mutex::new(LauncherView::Chooser),
            launcher_presentation: Mutex::new(LauncherPresentation::Standalone),
            zoom: Mutex::new(1.0),
            boot_sequence: AtomicU64::new(0),
            is_quitting: AtomicBool::new(false),
        }
    }

    /// Claims the next boot slot, superseding any boot already in flight.
    pub fn begin_boot(&self) -> BootToken {
        BootToken {
            id: self.boot_sequence.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }

    /// Whether `token` still represents the newest boot.
    pub fn is_current(&self, token: &BootToken) -> bool {
        self.boot_sequence.load(Ordering::SeqCst) == token.id
    }

    pub fn quitting(&self) -> bool {
        self.is_quitting.load(Ordering::SeqCst)
    }

    pub fn set_quitting(&self) {
        self.is_quitting.store(true, Ordering::SeqCst);
    }

    pub fn current_connection(&self) -> Option<CurrentConnection> {
        self.connection.read().ok().and_then(|guard| guard.clone())
    }

    pub fn set_connection(&self, connection: Option<CurrentConnection>) {
        if let Ok(mut guard) = self.connection.write() {
            *guard = connection;
        }
    }

    pub fn launcher_view(&self) -> LauncherView {
        self.launcher_view
            .lock()
            .map(|view| *view)
            .unwrap_or(LauncherView::Chooser)
    }

    pub fn set_launcher_view(&self, view: LauncherView) {
        if let Ok(mut guard) = self.launcher_view.lock() {
            *guard = view;
        }
    }

    pub fn launcher_presentation(&self) -> LauncherPresentation {
        self.launcher_presentation
            .lock()
            .map(|presentation| *presentation)
            .unwrap_or(LauncherPresentation::Standalone)
    }

    pub fn set_launcher_presentation(&self, presentation: LauncherPresentation) {
        if let Ok(mut guard) = self.launcher_presentation.lock() {
            *guard = presentation;
        }
    }

    /// Everything the launcher UI needs to render itself.
    ///
    /// Mirrors `buildLauncherSnapshot` in `main.ts`, including the field names,
    /// which `launcher.js` reads directly.
    pub fn launcher_snapshot(&self) -> LauncherSnapshot {
        let store = self.store.lock().expect("connection store poisoned");
        let persisted = store.snapshot();
        let profiles = store.list_profiles();
        drop(store);

        let connection = self.current_connection();

        LauncherSnapshot {
            initial_view: self.launcher_view(),
            active_profile_id: persisted.state.active_profile_id.clone(),
            has_current_connection: connection.is_some(),
            is_attached_launcher: self.launcher_presentation() == LauncherPresentation::Attached,
            current_connection_label: connection
                .as_ref()
                .map(|connection| self.describe_connection(connection)),
            state: persisted.state,
            profiles,
        }
    }

    /// Label for the "return to the session you already have open" affordance.
    fn describe_connection(&self, connection: &CurrentConnection) -> String {
        if connection.mode == ConnectionMode::LocalEmbedded {
            return "Return to Local".to_string();
        }

        let named = connection.profile_id.as_deref().and_then(|profile_id| {
            self.store
                .lock()
                .ok()
                .and_then(|store| store.get_profile(profile_id))
                .filter(|profile| profile.mode == ConnectionMode::RemoteExisting)
                .map(|profile| format!("Return to {}", profile.name))
        });
        if let Some(named) = named {
            return named;
        }

        url::Url::parse(&connection.start_url)
            .ok()
            .and_then(|url| url.host_str().map(|host| format!("Return to {host}")))
            .unwrap_or_else(|| "Return to Current Session".to_string())
    }

    /// Convenience wrapper so callers do not re-implement the local-profile case.
    pub fn profile_display_name(&self, profile_id: &str) -> String {
        if profile_id == LOCAL_PROFILE_ID {
            return "Local".to_string();
        }
        self.store
            .lock()
            .ok()
            .and_then(|store| store.get_profile(profile_id))
            .map(|profile| profile.name)
            .unwrap_or_else(|| profile_id.to_string())
    }
}

/// Opaque handle proving which boot attempt a piece of work belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootToken {
    id: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSnapshot {
    pub initial_view: LauncherView,
    pub active_profile_id: Option<String>,
    pub has_current_connection: bool,
    pub is_attached_launcher: bool,
    pub current_connection_label: Option<String>,
    pub state: ConnectionState,
    pub profiles: Vec<ConnectionProfile>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::profiles::{connections_file_path, SaveRemoteProfile};
    use crate::connection::window_policy::local_data_store_id;

    fn state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::open(connections_file_path(dir.path()));
        let paths = AppPaths {
            user_data: dir.path().to_path_buf(),
            app_root: dir.path().to_path_buf(),
            paperclip_home: dir.path().join("home"),
            packaged: false,
        };
        (dir, AppState::new(paths, store))
    }

    #[test]
    fn launcher_views_use_the_names_the_ui_switches_on() {
        let names: Vec<String> = [
            LauncherView::Chooser,
            LauncherView::RemoteForm,
            LauncherView::Saved,
            LauncherView::LocalBoot,
            LauncherView::Connecting,
            LauncherView::Error,
        ]
        .iter()
        .map(|view| serde_json::to_string(view).unwrap())
        .collect();

        assert_eq!(
            names,
            vec![
                "\"chooser\"",
                "\"remote-form\"",
                "\"saved\"",
                "\"local-boot\"",
                "\"connecting\"",
                "\"error\"",
            ]
        );
    }

    #[test]
    fn a_newer_boot_supersedes_an_older_one() {
        let (_dir, state) = state();

        let first = state.begin_boot();
        assert!(state.is_current(&first));

        let second = state.begin_boot();
        assert!(!state.is_current(&first), "the older boot must stand down");
        assert!(state.is_current(&second));
    }

    #[test]
    fn the_snapshot_uses_the_field_names_the_launcher_reads() {
        let (_dir, state) = state();
        let value = serde_json::to_value(state.launcher_snapshot()).unwrap();
        let object = value.as_object().unwrap();

        for key in [
            "initialView",
            "activeProfileId",
            "hasCurrentConnection",
            "isAttachedLauncher",
            "currentConnectionLabel",
            "state",
            "profiles",
        ] {
            assert!(object.contains_key(key), "snapshot is missing {key}");
        }

        // The synthetic local profile is always present.
        assert_eq!(object["profiles"].as_array().unwrap().len(), 1);
        assert_eq!(object["hasCurrentConnection"], false);
    }

    #[test]
    fn the_connection_label_names_local_remote_and_unsaved_sessions() {
        let (_dir, state) = state();

        state.set_connection(Some(CurrentConnection {
            mode: ConnectionMode::LocalEmbedded,
            profile_id: Some(LOCAL_PROFILE_ID.into()),
            start_url: "http://localhost:3100".into(),
            allowed_origin: "http://localhost:3100".into(),
            data_store_id: local_data_store_id(),
        }));
        assert_eq!(
            state.launcher_snapshot().current_connection_label.unwrap(),
            "Return to Local"
        );

        let profile = state
            .store
            .lock()
            .unwrap()
            .save_remote_profile(SaveRemoteProfile {
                name: Some("Home Server"),
                ..SaveRemoteProfile::new("https://home.example.com")
            })
            .unwrap();
        state.set_connection(Some(CurrentConnection {
            mode: ConnectionMode::RemoteExisting,
            profile_id: Some(profile.id),
            start_url: "https://home.example.com/".into(),
            allowed_origin: "https://home.example.com".into(),
            data_store_id: [1; 16],
        }));
        assert_eq!(
            state.launcher_snapshot().current_connection_label.unwrap(),
            "Return to Home Server"
        );

        // An unsaved remote falls back to its host.
        state.set_connection(Some(CurrentConnection {
            mode: ConnectionMode::RemoteExisting,
            profile_id: None,
            start_url: "https://ad-hoc.example.com/".into(),
            allowed_origin: "https://ad-hoc.example.com".into(),
            data_store_id: [2; 16],
        }));
        assert_eq!(
            state.launcher_snapshot().current_connection_label.unwrap(),
            "Return to ad-hoc.example.com"
        );
    }

    #[test]
    fn there_is_no_label_without_a_connection() {
        let (_dir, state) = state();
        assert_eq!(state.launcher_snapshot().current_connection_label, None);
    }
}
