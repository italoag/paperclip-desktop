//! Window construction and lifecycle.
//!
//! Ported from the `BrowserWindow` factories in `src/main.ts`.

pub mod gestures;
pub mod hardening;
pub mod launcher;
pub mod main_window;

/// Window background used before the webview paints, matching the launcher's
/// `body { background: #0a0a0a }`. Without it the window flashes white.
pub const BACKGROUND: tauri::window::Color = tauri::window::Color(0x0a, 0x0a, 0x0a, 0xff);
