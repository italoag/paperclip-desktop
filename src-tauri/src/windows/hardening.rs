//! Security hardening applied to webviews that render untrusted content.
//!
//! Electron denied every permission request explicitly:
//!
//! ```js
//! win.webContents.session.setPermissionRequestHandler((_wc, _p, cb) => cb(false));
//! ```
//!
//! Tauri exposes no equivalent hook. A phase-0 spike measured the resulting
//! behaviour of the wry WKWebView against a remote origin:
//!
//! | API             | Default    | With this script     |
//! |-----------------|------------|----------------------|
//! | `geolocation`   | denied     | denied               |
//! | `getUserMedia`  | **GRANTED**| `NotAllowedError`    |
//!
//! So geolocation is already safe but media capture is not: without this the
//! migration would silently regress into letting a remote origin open the
//! microphone. The script below restores parity with Electron.

/// Removes the media-capture APIs before any page script runs.
///
/// Installed with `initialization_script_for_all_frames` so that iframes are
/// covered too, and the properties are defined non-configurable/non-writable so
/// page code cannot restore the originals.
pub const DENY_MEDIA_CAPTURE: &str = r#"
(() => {
  const deny = (name) => () =>
    Promise.reject(new DOMException(name + ' is disabled by Paperclip Desktop', 'NotAllowedError'));
  const lock = (obj, prop, value) => {
    try { Object.defineProperty(obj, prop, { value, writable: false, configurable: false }); }
    catch (_) { /* already non-configurable */ }
  };
  if (navigator.mediaDevices) {
    lock(navigator.mediaDevices, 'getUserMedia', deny('getUserMedia'));
    lock(navigator.mediaDevices, 'getDisplayMedia', deny('getDisplayMedia'));
  }
  lock(navigator, 'getUserMedia', undefined);
  lock(navigator, 'webkitGetUserMedia', undefined);
})();
"#;
