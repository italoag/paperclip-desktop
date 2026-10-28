//! Two-finger back/forward swipe navigation on macOS.
//!
//! Electron emulated this by hand: it listened for the `swipe` window event and
//! drove `webContents.navigationHistory` (see the now-removed
//! `src/navigation-gestures.ts`).
//!
//! wry supports the native `WKWebView` behaviour through
//! `back_forward_navigation_gestures`, but Tauri does not expose that builder
//! option, so reach through to the platform webview instead. This is both
//! simpler and smoother than the emulated version.

#[cfg(target_os = "macos")]
use tauri::{Runtime, WebviewWindow};

#[cfg(target_os = "macos")]
pub fn enable_back_forward<R: Runtime>(window: &WebviewWindow<R>) {
    let label = window.label().to_string();
    let inner_label = label.clone();

    let result = window.with_webview(move |platform| {
        let webview = platform.inner() as *mut objc2_web_kit::WKWebView;
        // SAFETY: `inner()` hands back the live WKWebView owned by wry, and this
        // closure runs on the main thread while the window is alive.
        unsafe {
            match webview.as_ref() {
                Some(webview) => webview.setAllowsBackForwardNavigationGestures(true),
                None => log::warn!("[{inner_label}] null WKWebView; swipe gestures unavailable"),
            }
        }
    });

    if let Err(err) = result {
        log::warn!("[{label}] could not enable swipe gestures: {err}");
    }
}

#[cfg(not(target_os = "macos"))]
pub fn enable_back_forward<R: tauri::Runtime>(_window: &tauri::WebviewWindow<R>) {}
