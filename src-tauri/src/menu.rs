//! The native application menu.
//!
//! Port of `rebuildAppMenu` / `buildConnectionMenuItems` in `src/main.ts`.
//!
//! Tauri ships predefined items for the standard edit actions, but has no
//! equivalent of Electron's `reload`, `zoomIn`, `zoomOut`, `resetZoom` or
//! `toggleDevTools` roles, so those are built by hand against the focused
//! webview.

use std::sync::Arc;

use tauri::menu::{
    AboutMetadata, CheckMenuItem, Menu, MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder,
};
use tauri::{AppHandle, Manager, Runtime};

use crate::boot::{self, BootLocalOptions, RememberChoice};
use crate::state::{AppState, LauncherView};
use crate::windows::launcher::{self, NavigatePayload};

const DOCS_URL: &str = "https://docs.paperclip.ing/";
const RECENT_LIMIT: usize = 5;

const ZOOM_STEP: f64 = 0.1;
const ZOOM_MIN: f64 = 0.5;
const ZOOM_MAX: f64 = 3.0;

// Menu item ids. Connection entries for saved profiles use `connect:<id>`.
const ID_CHECK_UPDATES: &str = "check-updates";
const ID_LAUNCH_CHOOSER: &str = "launch-chooser";
const ID_CONNECT_LOCAL: &str = "connect-local";
const ID_MANAGE_CONNECTIONS: &str = "manage-connections";
const ID_ALWAYS_SHOW_CHOOSER: &str = "always-show-chooser";
const ID_RELOAD: &str = "reload";
const ID_FORCE_RELOAD: &str = "force-reload";
const ID_TOGGLE_DEVTOOLS: &str = "toggle-devtools";
const ID_ZOOM_RESET: &str = "zoom-reset";
const ID_ZOOM_IN: &str = "zoom-in";
const ID_ZOOM_OUT: &str = "zoom-out";
const ID_DOCS: &str = "docs";
const CONNECT_PREFIX: &str = "connect:";

pub fn build<R: Runtime>(app: &AppHandle<R>, state: &AppState) -> tauri::Result<Menu<R>> {
    let snapshot = state.launcher_snapshot();

    let app_menu = SubmenuBuilder::new(app, "Paperclip")
        .item(&PredefinedMenuItem::about(
            app,
            None,
            Some(AboutMetadata::default()),
        )?)
        .item(&MenuItem::with_id(
            app,
            ID_CHECK_UPDATES,
            "Check for Updates",
            true,
            None::<&str>,
        )?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, None)?)
        .build()?;

    let mut connection = SubmenuBuilder::new(app, "Connection")
        .item(&MenuItem::with_id(
            app,
            ID_LAUNCH_CHOOSER,
            "Launch Chooser",
            true,
            None::<&str>,
        )?)
        .item(&MenuItem::with_id(
            app,
            ID_CONNECT_LOCAL,
            "Connect Local",
            true,
            None::<&str>,
        )?)
        .item(&MenuItem::with_id(
            app,
            ID_MANAGE_CONNECTIONS,
            "Manage Connections",
            true,
            None::<&str>,
        )?)
        .item(&CheckMenuItem::with_id(
            app,
            ID_ALWAYS_SHOW_CHOOSER,
            "Always Show Chooser On Launch",
            true,
            snapshot.state.always_show_chooser,
            None::<&str>,
        )?);

    let recents = state
        .store
        .lock()
        .map(|store| store.recent_remote_profiles(RECENT_LIMIT))
        .unwrap_or_default();
    if !recents.is_empty() {
        connection = connection.separator();
        for profile in recents {
            let host = profile
                .remote_url
                .as_deref()
                .and_then(|url| url::Url::parse(url).ok())
                .and_then(|url| url.host_str().map(str::to_string))
                .unwrap_or_default();
            let label = if host.is_empty() {
                profile.name.clone()
            } else {
                format!("{} ({host})", profile.name)
            };
            connection = connection.item(&MenuItem::with_id(
                app,
                format!("{CONNECT_PREFIX}{}", profile.id),
                label,
                true,
                None::<&str>,
            )?);
        }
    }

    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;

    let view = SubmenuBuilder::new(app, "View")
        .item(&MenuItem::with_id(
            app,
            ID_RELOAD,
            "Reload",
            true,
            Some("CmdOrCtrl+R"),
        )?)
        .item(&MenuItem::with_id(
            app,
            ID_FORCE_RELOAD,
            "Force Reload",
            true,
            Some("CmdOrCtrl+Shift+R"),
        )?)
        .item(&MenuItem::with_id(
            app,
            ID_TOGGLE_DEVTOOLS,
            "Toggle Developer Tools",
            true,
            Some("CmdOrCtrl+Alt+I"),
        )?)
        .separator()
        .item(&MenuItem::with_id(
            app,
            ID_ZOOM_RESET,
            "Actual Size",
            true,
            Some("CmdOrCtrl+0"),
        )?)
        .item(&MenuItem::with_id(
            app,
            ID_ZOOM_IN,
            "Zoom In",
            true,
            Some("CmdOrCtrl+Plus"),
        )?)
        .item(&MenuItem::with_id(
            app,
            ID_ZOOM_OUT,
            "Zoom Out",
            true,
            Some("CmdOrCtrl+-"),
        )?)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;

    let help = SubmenuBuilder::new(app, "Help")
        .item(&MenuItem::with_id(
            app,
            ID_DOCS,
            "Documentation",
            true,
            None::<&str>,
        )?)
        .build()?;

    MenuBuilder::new(app)
        .item(&app_menu)
        .item(&connection.build()?)
        .item(&edit)
        .item(&view)
        .item(&help)
        .build()
}

/// Rebuilds and installs the menu so the checkbox and recents stay in sync.
pub fn refresh<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    match build(app, state) {
        Ok(menu) => {
            if let Err(error) = app.set_menu(menu) {
                log::warn!("could not install the application menu: {error}");
            }
        }
        Err(error) => log::warn!("could not build the application menu: {error}"),
    }
}

pub fn handle_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let state = app.state::<Arc<AppState>>().inner().clone();

    if let Some(profile_id) = id.strip_prefix(CONNECT_PREFIX) {
        let profile_id = profile_id.to_string();
        let _ = launcher::ensure(
            app,
            &state,
            NavigatePayload::connecting(
                "Opening verified remote...",
                state.profile_display_name(&profile_id),
            ),
        );
        tauri::async_runtime::spawn(boot::boot_saved_profile(
            app.clone(),
            profile_id,
            RememberChoice::default(),
        ));
        return;
    }

    match id {
        ID_LAUNCH_CHOOSER => {
            let _ = launcher::ensure(app, &state, NavigatePayload::view(LauncherView::Chooser));
        }
        ID_MANAGE_CONNECTIONS => {
            let _ = launcher::ensure(app, &state, NavigatePayload::view(LauncherView::Saved));
        }
        ID_CONNECT_LOCAL => {
            let _ = launcher::ensure(app, &state, NavigatePayload::view(LauncherView::LocalBoot));
            tauri::async_runtime::spawn(boot::boot_local(
                app.clone(),
                BootLocalOptions {
                    trigger: "menu",
                    ..Default::default()
                },
            ));
        }
        ID_ALWAYS_SHOW_CHOOSER => {
            let next = !state
                .store
                .lock()
                .map(|store| store.snapshot().state.always_show_chooser)
                .unwrap_or(true);
            if let Ok(mut store) = state.store.lock() {
                let _ = store.set_always_show_chooser(next);
            }
            launcher::send_state(app, &state);
            refresh(app, &state);
        }
        // Both do the same thing. Tauri exposes no cache-ignoring reload, and the
        // only alternative — `clear_all_browsing_data` — would also drop cookies
        // and local storage, which is not what "force reload" means anywhere else.
        //
        // This matters when shipping a patched UI asset: the server marks assets
        // `immutable` for a year, so neither item can pull a corrected bundle that
        // reuses its old filename. `scripts/patch-ui-cost-unit.mjs` renames the
        // file for exactly that reason.
        ID_RELOAD | ID_FORCE_RELOAD => {
            if let Some(window) = focused(app) {
                let _ = window.reload();
            }
        }
        ID_TOGGLE_DEVTOOLS => toggle_devtools(app),
        ID_ZOOM_RESET => set_zoom(app, &state, |_| 1.0),
        ID_ZOOM_IN => set_zoom(app, &state, |current| current + ZOOM_STEP),
        ID_ZOOM_OUT => set_zoom(app, &state, |current| current - ZOOM_STEP),
        ID_DOCS => {
            use tauri_plugin_opener::OpenerExt;
            if let Err(error) = app.opener().open_url(DOCS_URL, None::<&str>) {
                log::warn!("could not open the documentation: {error}");
            }
        }
        ID_CHECK_UPDATES => crate::updater::check_from_menu(app.clone()),
        other => log::debug!("unhandled menu item: {other}"),
    }
}

fn focused<R: Runtime>(app: &AppHandle<R>) -> Option<tauri::WebviewWindow<R>> {
    app.webview_windows()
        .into_values()
        .find(|window| window.is_focused().unwrap_or(false))
        .or_else(|| app.get_webview_window(crate::windows::main_window::LABEL))
}

/// The `tauri/devtools` feature is enabled unconditionally in `Cargo.toml`, so
/// the inspector is available in release builds too — matching the Electron
/// menu, which also offered it to users.
fn toggle_devtools<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = focused(app) else { return };

    if window.is_devtools_open() {
        window.close_devtools();
    } else {
        window.open_devtools();
    }
}

fn set_zoom<R: Runtime>(app: &AppHandle<R>, state: &AppState, next: impl Fn(f64) -> f64) {
    let Some(window) = focused(app) else { return };

    let Ok(mut zoom) = state.zoom.lock() else {
        return;
    };
    *zoom = next(*zoom).clamp(ZOOM_MIN, ZOOM_MAX);

    if let Err(error) = window.set_zoom(*zoom) {
        log::debug!("could not change zoom: {error}");
    }
}
