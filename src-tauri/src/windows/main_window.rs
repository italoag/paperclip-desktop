//! The window that renders Paperclip itself, local or remote.
//!
//! Port of `createMainWindow` + `applyWindowPolicy` in `src/main.ts`.
//!
//! Everything in here is a containment boundary. The window loads a web app we
//! do not control — for remote profiles, one on someone else's server — so it
//! gets: a hard same-origin navigation lock, no ability to open windows, no
//! media capture, and storage isolated to its own profile.

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

use crate::connection::window_policy::{is_navigation_allowed, should_open_externally};

pub const LABEL: &str = "main";

const WIDTH: f64 = 1440.0;
const HEIGHT: f64 = 900.0;
const MIN_WIDTH: f64 = 900.0;
const MIN_HEIGHT: f64 = 600.0;

pub struct MainWindowOptions {
    pub start_url: String,
    pub allowed_origin: String,
    pub data_store_id: [u8; 16],
}

/// Builds (or rebuilds) the main window pointed at `start_url`.
///
/// Any existing main window is destroyed first: a window's data store is fixed
/// at creation, so switching profiles genuinely requires a new one.
pub fn create<R: Runtime>(
    app: &AppHandle<R>,
    options: MainWindowOptions,
) -> tauri::Result<WebviewWindow<R>> {
    if let Some(existing) = app.get_webview_window(LABEL) {
        existing.destroy()?;
    }

    let url: url::Url = options
        .start_url
        .parse()
        .map_err(|_| tauri::Error::WebviewNotFound)?;

    let allowed_for_navigation = options.allowed_origin.clone();
    let allowed_for_new_window = options.allowed_origin.clone();
    let opener_handle = app.clone();
    let new_window_handle = app.clone();

    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url))
        .title("Paperclip")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .visible(false)
        .background_color(super::BACKGROUND)
        .initialization_script_for_all_frames(super::hardening::DENY_MEDIA_CAPTURE)
        // Replaces both `will-navigate` and `will-redirect`: returning false
        // cancels the navigation, and off-origin web links are handed to the
        // user's browser instead of being silently dropped.
        .on_navigation(move |url| {
            let target = url.as_str();
            if is_navigation_allowed(target, &allowed_for_navigation) {
                return true;
            }

            if should_open_externally(target, &allowed_for_navigation) {
                if let Err(error) = opener_handle.opener().open_url(target, None::<&str>) {
                    log::warn!("could not open {target} externally: {error}");
                }
            } else {
                log::warn!("blocked navigation to {target}");
            }
            false
        })
        // Replaces `setWindowOpenHandler`: never let the page spawn a window we
        // would not be applying this policy to.
        .on_new_window(move |url, _features| {
            let target = url.as_str();
            if should_open_externally(target, &allowed_for_new_window) {
                if let Err(error) = new_window_handle.opener().open_url(target, None::<&str>) {
                    log::warn!("could not open {target} externally: {error}");
                }
            }
            tauri::webview::NewWindowResponse::Deny
        });

    #[cfg(target_os = "macos")]
    {
        builder = builder.data_store_identifier(options.data_store_id);
    }

    let window = builder.build()?;
    super::gestures::enable_back_forward(&window);

    Ok(window)
}

/// Shows the window once its first paint is in, avoiding a white flash.
pub fn reveal<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    window.show()?;
    window.set_focus()
}
