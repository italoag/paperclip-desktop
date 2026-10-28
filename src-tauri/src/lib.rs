//! Paperclip Desktop — Tauri shell.
//!
//! This crate replaces the previous Electron main process. It owns window
//! lifecycle, the embedded Paperclip server child process, the connection
//! launcher, and the native application menu.
//!
//! The Paperclip product itself is *not* part of this crate: it ships as the
//! `@paperclipai/server` Node.js bundle that this shell supervises.

pub mod boot;
pub mod commands;
pub mod connection;
pub mod menu;
pub mod paths;
pub mod runtime_safety;
pub mod server;
pub mod state;
pub mod updater;
pub mod windows;

use std::sync::Arc;

use tauri::{AppHandle, Manager, RunEvent, Runtime, WindowEvent};

use crate::connection::profiles::{connections_file_path, ConnectionStore};
use crate::connection::types::LOCAL_PROFILE_ID;
use crate::runtime_safety::{Platform, RuntimeEnv};
use crate::state::{AppPaths, AppState, LauncherView};
use crate::windows::launcher::{self, NavigatePayload};
use crate::windows::main_window;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Registered first, per the plugin's requirements. A second launch must
        // never boot its own server against the same PAPERCLIP_HOME.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            focus_existing_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::launcher_bootstrap,
            commands::launcher_set_chooser_mode,
            commands::launcher_save_remote_profile,
            commands::launcher_duplicate_profile,
            commands::launcher_delete_profile,
            commands::launcher_verify_remote,
            commands::launcher_connect_local,
            commands::launcher_connect_remote,
            commands::launcher_connect_saved_profile,
            commands::launcher_show_chooser,
            commands::launcher_close_sheet,
            commands::launcher_report_content_height,
        ])
        .setup(|app| {
            let state = initialise(app.handle())?;
            app.manage(state.clone());

            menu::refresh(app.handle(), &state);
            install_signal_handlers(app.handle().clone());
            updater::start(app.handle().clone());
            start_session(app.handle().clone(), state);

            Ok(())
        })
        .on_menu_event(|app, event| menu::handle_event(app, event.id().as_ref()))
        .on_window_event(|window, event| {
            // Closing the launcher must not take the app with it while a
            // session is running.
            if let WindowEvent::Destroyed = event {
                log::debug!("window destroyed: {}", window.label());
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building the Tauri application")
        .run(|app, event| match event {
            // macOS: clicking the Dock icon with no window open.
            RunEvent::Reopen { .. } => focus_existing_window(app),
            RunEvent::ExitRequested { .. } => shutdown(app),
            RunEvent::Exit => shutdown(app),
            _ => {}
        });
}

/// Resolves paths, enforces the data-isolation guard, and opens the store.
fn initialise<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<AppState>, Box<dyn std::error::Error>> {
    let env = RuntimeEnv::from_process();
    let platform = Platform::CURRENT;
    let home = app.path().home_dir().unwrap_or_default();

    let user_data = paths::user_data_dir(&env, platform, &home)?;
    let paperclip_home =
        runtime_safety::resolve_paperclip_home(&env, &home, &user_data, &|path| path.exists())?;

    // Refuses to run against the real database when a smoke test asked for
    // isolation. Must happen before anything touches those directories.
    runtime_safety::assert_isolated_runtime_paths(
        &env,
        &user_data,
        &paperclip_home,
        &home,
        platform,
    )?;

    // Detected from the payload actually being on disk rather than from
    // `debug_assertions`: a release binary run straight out of `target/` is not
    // packaged, and treating it as such would point the server at a path that
    // does not exist.
    let resource_dir = app
        .path()
        .resource_dir()
        .ok()
        .filter(|dir| dir.join("app-server").is_dir());
    let packaged = resource_dir.is_some();
    let app_root = paths::app_root(resource_dir.as_deref());

    std::fs::create_dir_all(&user_data)?;
    log::info!(
        "user data: {} | PAPERCLIP_HOME: {} | app root: {}",
        user_data.display(),
        paperclip_home.display(),
        app_root.display()
    );

    // A server left behind by a crashed run would hold the port and the database.
    server::pid_file::kill_orphaned_server(&server::pid_file::pid_file_path(&user_data));

    // Adapters shipped inside the app are only loaded if they are listed in the
    // plugin store, so the store is reconciled with the bundle on every launch.
    server::adapter_plugins::register_bundled(&app_root, &paperclip_home);

    let store = ConnectionStore::open(connections_file_path(&user_data));

    Ok(Arc::new(AppState::new(
        AppPaths {
            user_data,
            app_root,
            paperclip_home,
            packaged,
        },
        store,
    )))
}

/// Opens whatever the user's saved preference says, or the chooser.
fn start_session<R: Runtime>(app: AppHandle<R>, state: Arc<AppState>) {
    let startup = state
        .store
        .lock()
        .ok()
        .and_then(|store| store.startup_profile_id());

    match startup {
        Some(profile_id) if profile_id == LOCAL_PROFILE_ID => {
            let _ = launcher::ensure(&app, &state, NavigatePayload::view(LauncherView::LocalBoot));
            tauri::async_runtime::spawn(boot::boot_local(
                app,
                boot::BootLocalOptions {
                    trigger: "startup",
                    ..Default::default()
                },
            ));
        }
        Some(profile_id) => {
            let label = state.profile_display_name(&profile_id);
            let _ = launcher::ensure(
                &app,
                &state,
                NavigatePayload::connecting("Opening verified remote...", label),
            );
            tauri::async_runtime::spawn(boot::boot_saved_profile(
                app,
                profile_id,
                boot::RememberChoice::default(),
            ));
        }
        None => {
            let _ = launcher::ensure(&app, &state, NavigatePayload::view(LauncherView::Chooser));
        }
    }
}

/// Brings the running instance forward, creating a window if none is left.
fn focus_existing_window<R: Runtime>(app: &AppHandle<R>) {
    for label in [main_window::LABEL, launcher::LABEL] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            return;
        }
    }

    if let Some(state) = app.try_state::<Arc<AppState>>() {
        let state = state.inner().clone();
        let _ = launcher::ensure(app, &state, NavigatePayload::view(LauncherView::Chooser));
    }
}

/// Reaps the server when the process is signalled rather than quit normally.
///
/// Tauri's `ExitRequested`/`Exit` events only fire for an orderly quit. A bare
/// `SIGTERM` — from `pkill`, a supervisor, a logout, or a system shutdown — kills
/// the shell outright, and without this the Node server and its embedded
/// PostgreSQL keep running, holding the port and the database. The Electron
/// build installed the same three handlers; dropping them silently broke the
/// "the server is shut down when you quit" guarantee.
#[cfg(unix)]
fn install_signal_handlers<R: Runtime>(app: AppHandle<R>) {
    use tokio::signal::unix::{signal, SignalKind};

    tauri::async_runtime::spawn(async move {
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(error) => return log::warn!("could not listen for SIGTERM: {error}"),
        };
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(stream) => stream,
            Err(error) => return log::warn!("could not listen for SIGINT: {error}"),
        };
        let mut hangup = match signal(SignalKind::hangup()) {
            Ok(stream) => stream,
            Err(error) => return log::warn!("could not listen for SIGHUP: {error}"),
        };

        let name = tokio::select! {
            _ = terminate.recv() => "SIGTERM",
            _ = interrupt.recv() => "SIGINT",
            _ = hangup.recv() => "SIGHUP",
        };

        log::info!("received {name}; stopping the embedded server before exiting");
        if let Some(state) = app.try_state::<Arc<AppState>>() {
            let state = state.inner().clone();
            state.set_quitting();
            boot::stop_current_server(&state).await;
        }
        app.exit(0);
    });
}

#[cfg(not(unix))]
fn install_signal_handlers<R: Runtime>(_app: AppHandle<R>) {}

/// Stops the embedded server before the process goes away.
///
/// Runs on the exit path rather than a window-close handler because the server
/// must outlive the windows: closing the main window on macOS keeps the app
/// running, and the session with it.
fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        return;
    };
    if state.quitting() {
        return;
    }
    state.set_quitting();

    let state = state.inner().clone();
    tauri::async_runtime::block_on(async move {
        boot::stop_current_server(&state).await;
    });
}
