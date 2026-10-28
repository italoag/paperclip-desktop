//! Connection boot flows.
//!
//! Port of `bootLocal`, `bootRemote` and `bootSavedProfile` in `src/main.ts`.
//!
//! # Re-entrancy
//!
//! Every one of these can be triggered again while an earlier attempt is still
//! in flight — the user clicking a different connection, the menu, a recovery
//! prompt. Each attempt therefore claims a [`BootToken`] and re-checks it after
//! every await; a superseded attempt tears down whatever it built instead of
//! installing a window over the newer one.

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

use crate::connection::health::{probe_local_server_health, DEFAULT_TIMEOUT as HEALTH_TIMEOUT};
use crate::connection::http::ReqwestProbe;
use crate::connection::preflight::{preflight_remote_connection, PreflightOptions};
use crate::connection::profiles::SaveRemoteProfile;
use crate::connection::types::{
    ConnectionMode, RemotePreflightFailureReason, RemotePreflightResult, LOCAL_PROFILE_ID,
};
use crate::connection::validate::normalize_remote_url;
use crate::connection::window_policy::{
    data_store_id_for_origin, data_store_id_for_profile, local_data_store_id,
};
use crate::server::agent_secret::{self, AgentJwtSecret};
use crate::server::boot_progress::{BootProgress, BootStep, OutputTail};
use crate::server::supervisor::{self, ServerConfig};
use crate::server::{logging, node, pid_file, ports};
use crate::state::{AppState, BootToken, CurrentConnection, LauncherView};
use crate::windows::launcher::{self, NavigatePayload};
use crate::windows::main_window::{self, MainWindowOptions};

/// How often the monitor notices that the server process is gone.
const EXIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How often the monitor asks the server whether it is healthy.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Default, Clone)]
pub struct RememberChoice {
    /// Whether the user actively made a choice this time round. Only then is the
    /// stored preference rewritten.
    pub explicit: bool,
    pub remember: bool,
}

#[derive(Debug, Clone)]
pub struct BootLocalOptions {
    pub remember: RememberChoice,
    pub force_restart: bool,
    /// What asked for this boot. Logged so a runaway boot loop can be traced to
    /// its source — a spawned task's backtrace does not include its spawner.
    pub trigger: &'static str,
}

impl Default for BootLocalOptions {
    fn default() -> Self {
        Self {
            remember: RememberChoice::default(),
            force_restart: false,
            trigger: "unspecified",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BootRemoteOptions {
    pub profile_id: Option<String>,
    pub remote_url: String,
    pub display_name: Option<String>,
    pub save_profile: bool,
    pub allow_insecure_http: bool,
    pub remember: RememberChoice,
}

// ---------------------------------------------------------------------------
// Local
// ---------------------------------------------------------------------------

pub async fn boot_local<R: Runtime>(app: AppHandle<R>, options: BootLocalOptions) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let token = state.begin_boot();
    log::info!(
        "boot_local: trigger={} token={token:?} force_restart={}",
        options.trigger,
        options.force_restart
    );

    // Already connected locally and nothing forced a restart: just come back to
    // the window that is already running.
    if !options.force_restart && has_live_local_session(&app, &state) {
        record_local_connection(&state, &options.remember);
        launcher::send_state(&app, &state);
        launcher::close(&app);
        if let Some(window) = app.get_webview_window(main_window::LABEL) {
            let _ = window.set_focus();
        }
        return;
    }

    let _ = launcher::ensure(&app, &state, NavigatePayload::view(LauncherView::LocalBoot));

    if let Err(error) = start_local(&app, &state, &token).await {
        // A superseded boot is not a failure the user should see.
        if state.is_current(&token) {
            launcher::send_connection_error(&app, "Failed to start local Paperclip", &error);
            launcher::send_navigate(&app, &state, NavigatePayload::view(LauncherView::Error));
        }
    }
}

async fn start_local<R: Runtime>(
    app: &AppHandle<R>,
    state: &Arc<AppState>,
    token: &BootToken,
) -> Result<(), String> {
    let mut progress = BootProgress::new();
    if let Some(status) = progress.advance(BootStep::Init, "Preparing environment...", 5) {
        launcher::send_boot_status(app, status);
    }

    let port = ports::find_free_port(ports::PREFERRED_PORT)
        .await
        .map_err(|error| error.to_string())?;
    if !state.is_current(token) {
        return Ok(());
    }

    if let Some(status) =
        progress.advance(BootStep::Database, "Launching embedded PostgreSQL...", 15)
    {
        launcher::send_boot_status(app, status);
    }

    let log_path = logging::log_file_path(&state.paths.user_data);
    let mut log = logging::open_for_append(&log_path).ok();
    if let Some(file) = log.as_mut() {
        use std::io::Write;
        let _ = write!(
            file,
            "{}",
            supervisor::log_banner(port, &crate::connection::profiles::now_iso8601())
        );
    }

    let config = ServerConfig {
        node_binary: node::find_node_binary(
            state
                .paths
                .packaged
                .then_some(state.paths.app_root.as_path()),
            dirs_home().as_deref(),
        ),
        entry_point: supervisor::entry_point(&state.paths.app_root, state.paths.packaged),
        working_dir: state.paths.app_root.clone(),
        port,
        paperclip_home: state.paths.paperclip_home.clone(),
        path_env: node::resolve_shell_path(
            dirs_home().as_deref(),
            std::env::var("PATH").ok().as_deref(),
        ),
        packaged: state.paths.packaged,
        // Generated on first run and reused afterwards. A failure here is not
        // fatal on its own — the server still boots and serves the UI — but every
        // agent write would 401, so it is worth surfacing in the log.
        agent_jwt_secret: agent_secret::resolve_or_create(&state.paths.user_data)
            .unwrap_or_else(|error| {
                log::error!(
                    "could not persist the agent JWT secret ({error}); using an ephemeral one, so agent tokens will not survive a restart"
                );
                AgentJwtSecret::ephemeral()
            }),
    };

    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut server = supervisor::spawn(&config, log, sender).map_err(|error| error.to_string())?;

    // Recorded before waiting so a crash during startup is still reapable by the
    // next launch.
    if let Some(pid) = server.pid {
        pid_file::write(&pid_file::pid_file_path(&state.paths.user_data), pid);
    }

    // Forward the supervisor's log-derived progress to the launcher.
    let forward_app = app.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(status) = receiver.recv().await {
            launcher::send_boot_status(&forward_app, status);
        }
    });

    let origin = format!("http://localhost:{port}");

    // Race the port against the process itself. Waiting only on the port — as an
    // earlier version of this did — means a server that dies immediately (a bad
    // bundle, a failed migration) leaves the user staring at "starting server"
    // for the full 60s timeout before getting a message that says nothing useful.
    // Electron caught this with a `child.on("exit")` handler; this is that.
    let outcome = tokio::select! {
        result = wait_with_ticks(app, &mut progress, port) => result,
        code = server.wait() => Err(match code {
            Some(code) => format!("The Paperclip server exited during startup (code {code})."),
            None => "The Paperclip server was terminated during startup.".to_string(),
        }),
    };
    forwarder.abort();

    if !state.is_current(token) {
        // Superseded mid-boot: drop what this attempt started and leave whatever
        // the newer boot installed alone.
        server.shutdown().await;
        return Ok(());
    }

    if let Err(error) = outcome {
        let tail = server.tail(OutputTail::DIALOG_LINES);
        server.shutdown().await;
        pid_file::clear(&pid_file::pid_file_path(&state.paths.user_data));

        let detail = if tail.is_empty() {
            format!("{error}\n\nLog: {}", log_path.display())
        } else {
            format!("{error}\n\nLog: {}\n\n{tail}", log_path.display())
        };
        return Err(detail);
    }

    let previous = state.server.lock().await.replace(server);

    if let Some(status) = progress.advance(BootStep::Server, "Server is ready", 70) {
        launcher::send_boot_status(app, status);
    }
    if let Some(status) = progress.advance(BootStep::Ready, "Loading the UI...", 80) {
        launcher::send_boot_status(app, status);
    }

    let window = main_window::create(
        app,
        MainWindowOptions {
            start_url: origin.clone(),
            allowed_origin: origin.clone(),
            data_store_id: local_data_store_id(),
        },
    )
    .map_err(|error| error.to_string())?;

    if !state.is_current(token) {
        let _ = window.destroy();
        stop_current_server(state).await;
        return Ok(());
    }

    // The boot succeeded, so the server it replaced is now dead weight.
    if let Some(mut previous) = previous {
        if !previous.has_exited() {
            previous.shutdown().await;
        }
    }

    let _ = main_window::reveal(&window);

    state.set_connection(Some(CurrentConnection {
        mode: ConnectionMode::LocalEmbedded,
        profile_id: Some(LOCAL_PROFILE_ID.to_string()),
        start_url: origin.clone(),
        allowed_origin: origin.clone(),
        data_store_id: local_data_store_id(),
    }));

    if let Some(status) = progress.advance(BootStep::Ready, "Ready!", 100) {
        launcher::send_boot_status(app, status);
    }
    launcher::send_state(app, state);
    launcher::close(app);

    spawn_monitor(app.clone(), state.clone(), origin);

    Ok(())
}

/// Waits for the port while keeping the progress bar moving.
async fn wait_with_ticks<R: Runtime>(
    app: &AppHandle<R>,
    progress: &mut BootProgress,
    port: u16,
) -> Result<(), String> {
    if let Some(status) = progress.advance(BootStep::Server, "Waiting for server...", 45) {
        launcher::send_boot_status(app, status);
    }

    let deadline = tokio::time::Instant::now() + ports::STARTUP_TIMEOUT;
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    ticker.tick().await; // The first tick completes immediately.

    loop {
        tokio::select! {
            listening = ports::is_port_in_use(port) => {
                if listening {
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "Server did not start within {}ms",
                        ports::STARTUP_TIMEOUT.as_millis()
                    ));
                }
                tokio::time::sleep(ports::POLL_INTERVAL).await;
            }
            _ = ticker.tick() => {
                if let Some(status) = progress.tick() {
                    launcher::send_boot_status(app, status);
                }
            }
        }
    }
}

fn has_live_local_session<R: Runtime>(app: &AppHandle<R>, state: &AppState) -> bool {
    state
        .current_connection()
        .is_some_and(|connection| connection.mode == ConnectionMode::LocalEmbedded)
        && app.get_webview_window(main_window::LABEL).is_some()
}

fn record_local_connection(state: &AppState, remember: &RememberChoice) {
    if let Ok(mut store) = state.store.lock() {
        let _ = store.record_connection_result(LOCAL_PROFILE_ID, None, None);
        if remember.explicit {
            let _ = store.set_remembered_profile(Some(LOCAL_PROFILE_ID), remember.remember);
        }
    }
}

// ---------------------------------------------------------------------------
// Remote
// ---------------------------------------------------------------------------

pub async fn boot_remote<R: Runtime>(app: AppHandle<R>, options: BootRemoteOptions) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let token = state.begin_boot();

    let normalized = match normalize_remote_url(&options.remote_url) {
        Ok(normalized) if normalized.insecure_transport && !options.allow_insecure_http => {
            fail_remote(
                &app,
                &state,
                LauncherView::RemoteForm,
                "Invalid remote URL",
                "HTTP remotes require confirming that you want to allow an insecure connection.",
            );
            return;
        }
        Ok(normalized) => normalized,
        Err(error) => {
            fail_remote(
                &app,
                &state,
                LauncherView::RemoteForm,
                "Invalid remote URL",
                error.to_string(),
            );
            return;
        }
    };

    let _ = launcher::ensure(
        &app,
        &state,
        NavigatePayload::connecting("Opening verified remote...", &normalized.normalized_url),
    );

    let Ok(probe) = ReqwestProbe::new() else {
        fail_remote(
            &app,
            &state,
            LauncherView::Error,
            "Could not verify remote",
            "The HTTP client could not be initialised.",
        );
        return;
    };

    let local_version = resolve_local_server_version(&state);
    let result = preflight_remote_connection(
        &probe,
        PreflightOptions {
            remote_url: &normalized.normalized_url,
            local_server_version: local_version.as_deref(),
            timeout: crate::connection::preflight::DEFAULT_TIMEOUT,
        },
    )
    .await;

    if !state.is_current(&token) {
        return;
    }

    if !result.ok {
        if let Some(profile_id) = options.profile_id.as_deref() {
            if let Ok(mut store) = state.store.lock() {
                let _ = store.record_remote_health(profile_id, &result, None);
            }
            launcher::send_state(&app, &state);
        }
        fail_remote(
            &app,
            &state,
            LauncherView::Error,
            remote_error_title(&result),
            result
                .detail
                .clone()
                .unwrap_or_else(|| "Could not verify the selected remote.".to_string()),
        );
        return;
    }

    // Persist before opening the window so a crash cannot lose the profile.
    let saved_profile = persist_remote_profile(&state, &options, &result);
    if let Some(profile) = saved_profile.as_deref() {
        if let Ok(mut store) = state.store.lock() {
            let _ = store.record_remote_health(profile, &result, None);
        }
    }
    if options.remember.explicit {
        if let Ok(mut store) = state.store.lock() {
            let _ = store.set_remembered_profile(
                saved_profile.as_deref().or(options.profile_id.as_deref()),
                options.remember.remember,
            );
        }
    }
    launcher::send_state(&app, &state);

    let profile_id = saved_profile.or(options.profile_id.clone());
    let data_store_id = match profile_id.as_deref() {
        Some(profile_id) => data_store_id_for_profile(profile_id),
        None => data_store_id_for_origin(&result.origin),
    };

    launcher::send_navigate(
        &app,
        &state,
        NavigatePayload::connecting(connecting_label(&result), &result.normalized_url),
    );

    let window = match main_window::create(
        &app,
        MainWindowOptions {
            start_url: result.normalized_url.clone(),
            allowed_origin: result.origin.clone(),
            data_store_id,
        },
    ) {
        Ok(window) => window,
        Err(error) => {
            fail_remote(
                &app,
                &state,
                LauncherView::Error,
                "Failed to open remote Paperclip",
                error.to_string(),
            );
            return;
        }
    };

    if !state.is_current(&token) {
        let _ = window.destroy();
        return;
    }

    let _ = main_window::reveal(&window);
    launcher::close(&app);

    // Leaving local mode means the embedded server is no longer needed.
    let was_local = state
        .current_connection()
        .is_some_and(|connection| connection.mode == ConnectionMode::LocalEmbedded);
    if was_local {
        stop_current_server(&state).await;
    }

    state.set_connection(Some(CurrentConnection {
        mode: ConnectionMode::RemoteExisting,
        profile_id: profile_id.clone(),
        start_url: result.normalized_url.clone(),
        allowed_origin: result.origin.clone(),
        data_store_id,
    }));

    if let Some(profile_id) = profile_id.as_deref() {
        if let Ok(mut store) = state.store.lock() {
            let _ = store.record_connection_result(profile_id, Some(&result), None);
        }
    }
    launcher::send_state(&app, &state);
}

pub async fn boot_saved_profile<R: Runtime>(
    app: AppHandle<R>,
    profile_id: String,
    remember: RememberChoice,
) {
    if profile_id == LOCAL_PROFILE_ID {
        boot_local(
            app,
            BootLocalOptions {
                remember,
                force_restart: false,
                trigger: "saved-profile",
            },
        )
        .await;
        return;
    }

    let state = app.state::<Arc<AppState>>().inner().clone();
    let profile = state
        .store
        .lock()
        .ok()
        .and_then(|store| store.get_profile(&profile_id));

    let Some(profile) = profile.filter(|profile| profile.mode == ConnectionMode::RemoteExisting)
    else {
        fail_remote(
            &app,
            &state,
            LauncherView::Error,
            "Unknown connection",
            format!("Saved profile {profile_id} no longer exists."),
        );
        return;
    };
    let Some(remote_url) = profile.remote_url.clone() else {
        fail_remote(
            &app,
            &state,
            LauncherView::Error,
            "Unknown connection",
            "That saved profile has no address.",
        );
        return;
    };

    boot_remote(
        app,
        BootRemoteOptions {
            profile_id: Some(profile_id),
            remote_url,
            display_name: Some(profile.name),
            save_profile: false,
            allow_insecure_http: profile.allow_insecure_http == Some(true),
            remember,
        },
    )
    .await;
}

fn persist_remote_profile(
    state: &AppState,
    options: &BootRemoteOptions,
    result: &RemotePreflightResult,
) -> Option<String> {
    let mut store = state.store.lock().ok()?;

    if let Some(profile_id) = options.profile_id.as_deref() {
        let _ = store.sync_remote_profile_url(
            profile_id,
            &result.normalized_url,
            result.insecure_transport,
            None,
        );
        return store.get_profile(profile_id).map(|profile| profile.id);
    }

    if !options.save_profile && !options.remember.remember {
        return None;
    }

    store
        .save_remote_profile(SaveRemoteProfile {
            profile_id: None,
            name: options.display_name.as_deref(),
            remote_url: &result.normalized_url,
            allow_insecure_http: result.insecure_transport,
            now: None,
        })
        .ok()
        .map(|profile| profile.id)
}

fn connecting_label(result: &RemotePreflightResult) -> &'static str {
    use crate::connection::types::{BootstrapStatus, SessionState};

    if result.bootstrap_status == Some(BootstrapStatus::BootstrapPending) {
        "Opening remote setup..."
    } else if result.session_state == SessionState::SignedOut {
        "Opening remote sign-in..."
    } else {
        "Opening verified remote..."
    }
}

fn remote_error_title(result: &RemotePreflightResult) -> &'static str {
    match result.reason {
        Some(RemotePreflightFailureReason::UnsupportedLocalTrusted) => "Remote not eligible",
        Some(RemotePreflightFailureReason::NotPaperclip) => "Non-Paperclip endpoint",
        Some(RemotePreflightFailureReason::TlsError) => "TLS validation failed",
        Some(RemotePreflightFailureReason::AuthNotReady) => "Remote auth is not ready",
        _ => "Could not verify remote",
    }
}

fn fail_remote<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    return_view: LauncherView,
    title: impl Into<String>,
    detail: impl Into<String>,
) {
    let _ = launcher::ensure(app, state, NavigatePayload::view(return_view));
    launcher::send_connection_error(app, title, detail);
    launcher::send_navigate(app, state, NavigatePayload::view(LauncherView::Error));
}

// ---------------------------------------------------------------------------
// Server lifecycle helpers
// ---------------------------------------------------------------------------

pub async fn stop_current_server(state: &AppState) {
    let server = state.server.lock().await.take();
    if let Some(server) = server {
        server.shutdown().await;
    }
    pid_file::clear(&pid_file::pid_file_path(&state.paths.user_data));
}

async fn server_tail(state: &AppState) -> String {
    state
        .server
        .lock()
        .await
        .as_ref()
        .map(|server| server.tail(OutputTail::DIALOG_LINES))
        .unwrap_or_default()
}

/// Watches a local session for a server that dies or stops answering.
///
/// The original only polled `/api/health` every 30s. Checking the process
/// itself every 5s as well means a crash surfaces in seconds rather than
/// half a minute.
fn spawn_monitor<R: Runtime>(app: AppHandle<R>, state: Arc<AppState>, origin: String) {
    tokio::spawn(async move {
        let probe = match ReqwestProbe::new() {
            Ok(probe) => probe,
            Err(error) => {
                log::warn!("local server monitor disabled: {error}");
                return;
            }
        };

        let mut since_health_check = Duration::ZERO;

        loop {
            tokio::time::sleep(EXIT_POLL_INTERVAL).await;
            since_health_check += EXIT_POLL_INTERVAL;

            if state.quitting() || !still_local(&state, &origin) {
                return;
            }

            let exited = {
                let mut guard = state.server.lock().await;
                match guard.as_mut() {
                    Some(server) => server.has_exited(),
                    // Nothing to watch any more.
                    None => return,
                }
            };
            if exited {
                report_server_lost(
                    &app,
                    &state,
                    "The embedded Paperclip server stopped unexpectedly.",
                )
                .await;
                return;
            }

            if since_health_check >= HEALTH_POLL_INTERVAL {
                since_health_check = Duration::ZERO;
                let health = probe_local_server_health(&probe, &origin, HEALTH_TIMEOUT).await;
                if !health.ok && still_local(&state, &origin) {
                    report_server_lost(
                        &app,
                        &state,
                        "The embedded Paperclip server is no longer responding.",
                    )
                    .await;
                    return;
                }
            }
        }
    });
}

fn still_local(state: &AppState, origin: &str) -> bool {
    state.current_connection().is_some_and(|connection| {
        connection.mode == ConnectionMode::LocalEmbedded && connection.allowed_origin == origin
    })
}

async fn report_server_lost<R: Runtime>(app: &AppHandle<R>, state: &AppState, message: &str) {
    let detail = server_tail(state).await;
    log::error!("{message}\n{detail}");

    let _ = launcher::ensure(app, state, NavigatePayload::view(LauncherView::Error));
    launcher::send_connection_error(
        app,
        message,
        if detail.is_empty() {
            "Restart the local server to continue.".to_string()
        } else {
            format!("{detail}\n\nRestart the local server to continue.")
        },
    );
    launcher::send_navigate(app, state, NavigatePayload::view(LauncherView::Error));
}

/// Version of the bundled server, used to warn about remote/local drift.
fn resolve_local_server_version(state: &AppState) -> Option<String> {
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

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
