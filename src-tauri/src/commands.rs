//! IPC surface exposed to the launcher window.
//!
//! Port of the `ipcMain.handle` registrations in `src/main.ts`. Only the
//! launcher window is granted these (see `capabilities/launcher.json`); the
//! window rendering Paperclip itself gets no IPC at all.
//!
//! Connect commands return as soon as the boot is *scheduled*, matching the
//! original's `void bootLocal(...); return { started: true }` — the launcher
//! tracks progress through events, not the command's return value.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State};

use crate::boot::{self, BootLocalOptions, BootRemoteOptions, RememberChoice};
use crate::connection::profiles::SaveRemoteProfile;
use crate::connection::types::{ConnectionMode, RemotePreflightResult};
use crate::state::{AppState, LauncherSnapshot, LauncherView};
use crate::windows::launcher::{self, NavigatePayload};
use crate::windows::main_window;

type Result<T> = std::result::Result<T, String>;

/// Rejects any invocation that did not come from the launcher window.
///
/// # Why this is necessary
///
/// Tauri's capability system governs **plugin** commands only. Commands defined
/// by the application itself are reachable from any webview that has the IPC
/// bridge — and Tauri injects `__TAURI_INTERNALS__` into *every* webview it
/// creates, including the one rendering remote Paperclip content
/// (`tauri/src/manager/webview.rs` pushes those scripts with no origin check).
///
/// Electron gave that window `sandbox: true` and no preload, so it had no IPC
/// surface whatsoever. Without this guard the migration would hand a remote
/// origin — or an XSS inside one — the ability to enumerate every saved profile
/// via `launcher_bootstrap` or delete them via `launcher_delete_profile`.
fn only_launcher<R: Runtime>(webview: &tauri::Webview<R>, command: &str) -> Result<()> {
    let label = webview.label();
    if label == launcher::LABEL {
        return Ok(());
    }

    log::warn!("rejected `{command}` invoked from webview `{label}`");
    Err("This command is not available from this window.".to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct Started {
    pub started: bool,
}

impl Started {
    fn yes() -> Self {
        Self { started: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProfilePayload {
    pub profile_id: Option<String>,
    pub name: Option<String>,
    pub remote_url: String,
    #[serde(default)]
    pub allow_insecure_http: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyRemotePayload {
    pub remote_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectLocalPayload {
    #[serde(default)]
    pub remember_choice: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRemotePayload {
    pub profile_id: Option<String>,
    pub remote_url: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub save_profile: bool,
    #[serde(default)]
    pub remember_choice: bool,
    #[serde(default)]
    pub allow_insecure_http: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectSavedProfilePayload {
    pub profile_id: String,
    #[serde(default)]
    pub remember_choice: bool,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn launcher_bootstrap<R: Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, Arc<AppState>>,
) -> Result<LauncherSnapshot> {
    only_launcher(&webview, "launcher_bootstrap")?;

    let snapshot = state.launcher_snapshot();
    // First call the launcher UI makes, so it doubles as proof the window
    // rendered and the IPC bridge is wired up.
    log::info!(
        "launcher ready: view={:?}, profiles={}",
        snapshot.initial_view,
        snapshot.profiles.len()
    );
    Ok(snapshot)
}

#[tauri::command]
pub fn launcher_set_chooser_mode<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    state: State<'_, Arc<AppState>>,
    mode: ConnectionMode,
) -> Result<LauncherSnapshot> {
    only_launcher(&webview, "launcher_set_chooser_mode")?;
    state
        .store
        .lock()
        .map_err(|_| "connection store is unavailable".to_string())?
        .set_chooser_mode(mode)
        .map_err(|error| error.to_string())?;

    launcher::send_state(&app, &state);
    Ok(state.launcher_snapshot())
}

#[tauri::command]
pub fn launcher_save_remote_profile<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    state: State<'_, Arc<AppState>>,
    payload: SaveProfilePayload,
) -> Result<LauncherSnapshot> {
    only_launcher(&webview, "launcher_save_remote_profile")?;
    state
        .store
        .lock()
        .map_err(|_| "connection store is unavailable".to_string())?
        .save_remote_profile(SaveRemoteProfile {
            profile_id: payload.profile_id.as_deref(),
            name: payload.name.as_deref(),
            remote_url: &payload.remote_url,
            allow_insecure_http: payload.allow_insecure_http,
            now: None,
        })
        .map_err(|error| error.to_string())?;

    launcher::send_state(&app, &state);
    Ok(state.launcher_snapshot())
}

#[tauri::command]
pub fn launcher_duplicate_profile<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    state: State<'_, Arc<AppState>>,
    profile_id: String,
) -> Result<LauncherSnapshot> {
    only_launcher(&webview, "launcher_duplicate_profile")?;
    state
        .store
        .lock()
        .map_err(|_| "connection store is unavailable".to_string())?
        .duplicate_remote_profile(&profile_id, None)
        .map_err(|error| error.to_string())?;

    launcher::send_state(&app, &state);
    Ok(state.launcher_snapshot())
}

/// Deletes a profile *and* the browsing data isolated to it.
///
/// Dropping the record without dropping the data store would leave the remote's
/// cookies on disk, so re-adding the same host later would silently resurrect a
/// session the user believed they had deleted.
#[tauri::command]
pub async fn launcher_delete_profile<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    state: State<'_, Arc<AppState>>,
    profile_id: String,
) -> Result<LauncherSnapshot> {
    only_launcher(&webview, "launcher_delete_profile")?;

    #[cfg(target_os = "macos")]
    {
        use crate::connection::window_policy::data_store_id_for_profile;
        let uuid = data_store_id_for_profile(&profile_id);
        if let Err(error) = app.remove_data_store(uuid).await {
            // Non-fatal: the profile still has to disappear from the list.
            log::warn!("could not remove the data store for {profile_id}: {error}");
        }
    }

    state
        .store
        .lock()
        .map_err(|_| "connection store is unavailable".to_string())?
        .delete_remote_profile(&profile_id)
        .map_err(|error| error.to_string())?;

    // Keep a live session usable, but detach it from the profile that is gone.
    if let Some(connection) = state.current_connection() {
        if connection.profile_id.as_deref() == Some(profile_id.as_str()) {
            state.set_connection(Some(crate::state::CurrentConnection {
                profile_id: None,
                ..connection
            }));
        }
    }

    launcher::send_state(&app, &state);
    Ok(state.launcher_snapshot())
}

// ---------------------------------------------------------------------------
// Verification and connection
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn launcher_verify_remote<R: Runtime>(
    webview: tauri::Webview<R>,
    state: State<'_, Arc<AppState>>,
    payload: VerifyRemotePayload,
) -> Result<RemotePreflightResult> {
    only_launcher(&webview, "launcher_verify_remote")?;
    use crate::connection::http::ReqwestProbe;
    use crate::connection::preflight::{
        preflight_remote_connection, PreflightOptions, DEFAULT_TIMEOUT,
    };

    let probe = ReqwestProbe::new().map_err(|error| error.to_string())?;
    let local_version = local_server_version(&state);

    Ok(preflight_remote_connection(
        &probe,
        PreflightOptions {
            remote_url: &payload.remote_url,
            local_server_version: local_version.as_deref(),
            timeout: DEFAULT_TIMEOUT,
        },
    )
    .await)
}

#[tauri::command]
pub fn launcher_connect_local<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    payload: ConnectLocalPayload,
) -> Result<Started> {
    only_launcher(&webview, "launcher_connect_local")?;
    tauri::async_runtime::spawn(boot::boot_local(
        app,
        BootLocalOptions {
            remember: RememberChoice {
                explicit: true,
                remember: payload.remember_choice,
            },
            force_restart: false,
            trigger: "launcher-ui",
        },
    ));
    Ok(Started::yes())
}

#[tauri::command]
pub fn launcher_connect_remote<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    payload: ConnectRemotePayload,
) -> Result<Started> {
    only_launcher(&webview, "launcher_connect_remote")?;
    tauri::async_runtime::spawn(boot::boot_remote(
        app,
        BootRemoteOptions {
            profile_id: payload.profile_id,
            remote_url: payload.remote_url,
            display_name: payload.display_name,
            save_profile: payload.save_profile,
            allow_insecure_http: payload.allow_insecure_http,
            remember: RememberChoice {
                explicit: true,
                remember: payload.remember_choice,
            },
        },
    ));
    Ok(Started::yes())
}

#[tauri::command]
pub fn launcher_connect_saved_profile<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    payload: ConnectSavedProfilePayload,
) -> Result<Started> {
    only_launcher(&webview, "launcher_connect_saved_profile")?;
    tauri::async_runtime::spawn(boot::boot_saved_profile(
        app,
        payload.profile_id,
        RememberChoice {
            explicit: true,
            remember: payload.remember_choice,
        },
    ));
    Ok(Started::yes())
}

// ---------------------------------------------------------------------------
// Launcher window control
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn launcher_show_chooser<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    state: State<'_, Arc<AppState>>,
) -> Result<LauncherSnapshot> {
    only_launcher(&webview, "launcher_show_chooser")?;
    launcher::ensure(&app, &state, NavigatePayload::view(LauncherView::Chooser))
        .map_err(|error| error.to_string())?;
    Ok(state.launcher_snapshot())
}

#[tauri::command]
pub fn launcher_close_sheet<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
) -> Result<()> {
    only_launcher(&webview, "launcher_close_sheet")?;
    launcher::close(&app);
    if let Some(window) = app.get_webview_window(main_window::LABEL) {
        window.set_focus().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn launcher_report_content_height<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    height: f64,
) -> Result<()> {
    only_launcher(&webview, "launcher_report_content_height")?;
    let Some(window) = app.get_webview_window(launcher::LABEL) else {
        return Ok(());
    };
    launcher::apply_content_height(&window, height).map_err(|error| error.to_string())
}

fn local_server_version(state: &AppState) -> Option<String> {
    let manifest = if state.paths.packaged {
        state.paths.app_root.join("server").join("package.json")
    } else {
        state
            .paths
            .app_root
            .join("node_modules")
            .join("@paperclipai")
            .join("server")
            .join("package.json")
    };

    let raw = std::fs::read_to_string(manifest).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed.get("version")?.as_str().map(str::to_string)
}
