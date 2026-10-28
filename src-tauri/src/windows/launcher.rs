//! The connection launcher window and the events that drive it.
//!
//! Port of `createLauncherWindow`, `ensureLauncherWindow` and the
//! `sendLauncher*` helpers in `src/main.ts`. Unlike the Electron version the
//! markup is a static file under `ui/` instead of a string written to a temp
//! file on every launch.

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::server::boot_progress::BootStatus;
use crate::state::{AppState, LauncherPresentation, LauncherView};

pub const LABEL: &str = "launcher";

const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 620.0;
const MIN_HEIGHT: f64 = 400.0;
/// Bounds the height the UI can request, mirroring the original's clamp against
/// the display work area.
pub const MAX_CONTENT_HEIGHT_MARGIN: f64 = 96.0;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigatePayload {
    pub view: LauncherView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl NavigatePayload {
    pub fn view(view: LauncherView) -> Self {
        Self {
            view,
            label: None,
            url: None,
        }
    }

    pub fn connecting(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            view: LauncherView::Connecting,
            label: Some(label.into()),
            url: Some(url.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionErrorPayload {
    pub title: String,
    pub detail: String,
}

fn emit<R: Runtime, P: Serialize + Clone>(app: &AppHandle<R>, event: &str, payload: P) {
    if let Err(error) = app.emit_to(LABEL, event, payload) {
        // Perfectly normal once the launcher has been dismissed.
        log::debug!("could not emit {event}: {error}");
    }
}

pub fn send_state<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    emit(app, "launcher:state-changed", state.launcher_snapshot());
}

pub fn send_navigate<R: Runtime>(app: &AppHandle<R>, state: &AppState, payload: NavigatePayload) {
    state.set_launcher_view(payload.view);
    emit(app, "launcher:navigate", payload);
}

pub fn send_boot_status<R: Runtime>(app: &AppHandle<R>, status: BootStatus) {
    emit(app, "launcher:boot-status", status);
}

pub fn send_connection_error<R: Runtime>(
    app: &AppHandle<R>,
    title: impl Into<String>,
    detail: impl Into<String>,
) {
    emit(
        app,
        "launcher:connection-error",
        ConnectionErrorPayload {
            title: title.into(),
            detail: detail.into(),
        },
    );
}

pub fn request_content_height_sync<R: Runtime>(app: &AppHandle<R>) {
    emit(app, "launcher:request-content-height-sync", ());
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// Creates the launcher if needed, then navigates it to `view`.
///
/// The presentation depends on whether a session is already open: standalone at
/// startup, and a child window over the main one afterwards. Switching between
/// the two requires rebuilding the window, exactly as in the original.
pub fn ensure<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    payload: NavigatePayload,
) -> tauri::Result<WebviewWindow<R>> {
    let desired = if app.get_webview_window(super::main_window::LABEL).is_some() {
        LauncherPresentation::Attached
    } else {
        LauncherPresentation::Standalone
    };

    let existing = app.get_webview_window(LABEL);
    let must_recreate = existing.is_none() || state.launcher_presentation() != desired;

    let window = if must_recreate {
        if let Some(existing) = existing {
            existing.destroy()?;
        }
        build(app, desired)?
    } else {
        let window = existing.expect("checked above");
        window.show()?;
        window.set_focus()?;
        window
    };

    state.set_launcher_presentation(desired);
    send_state(app, state);
    send_navigate(app, state, payload);

    Ok(window)
}

fn build<R: Runtime>(
    app: &AppHandle<R>,
    presentation: LauncherPresentation,
) -> tauri::Result<WebviewWindow<R>> {
    let attached = presentation == LauncherPresentation::Attached;

    #[allow(unused_mut)]
    let mut builder =
        WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("launcher.html".into()))
            .title("Paperclip")
            .inner_size(WIDTH, HEIGHT)
            .min_inner_size(WIDTH, MIN_HEIGHT)
            .resizable(false)
            .maximizable(false)
            .minimizable(!attached)
            // Shown only once the UI reports its measured height, which avoids the
            // window appearing at the wrong size and then snapping.
            .visible(false)
            .background_color(super::BACKGROUND);

    #[cfg(target_os = "macos")]
    {
        builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay);
    }

    // Tauri has no modal windows. A parented, always-on-top window is the
    // closest equivalent to Electron's `modal: true` sheet.
    if attached {
        if let Some(parent) = app.get_webview_window(super::main_window::LABEL) {
            builder = builder.parent(&parent)?.always_on_top(true);
        }
    }

    builder.build()
}

pub fn close<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(LABEL) {
        if let Err(error) = window.destroy() {
            log::debug!("could not close the launcher: {error}");
        }
    }
}

/// Applies a height requested by the UI, clamped to the display's work area.
pub fn apply_content_height<R: Runtime>(
    window: &WebviewWindow<R>,
    height: f64,
) -> tauri::Result<()> {
    let available = window
        .current_monitor()?
        .map(|monitor| {
            let scale = monitor.scale_factor();
            monitor.size().to_logical::<f64>(scale).height - MAX_CONTENT_HEIGHT_MARGIN
        })
        .unwrap_or(f64::MAX);

    let target = height.clamp(MIN_HEIGHT, available.max(MIN_HEIGHT));
    let current = window
        .inner_size()?
        .to_logical::<f64>(window.scale_factor()?);

    // Ignore sub-pixel noise from the ResizeObserver.
    if (current.height - target).abs() > 2.0 {
        window.set_size(tauri::LogicalSize::new(current.width, target))?;
    }

    if !window.is_visible()? {
        window.show()?;
        window.set_focus()?;
    }

    Ok(())
}
