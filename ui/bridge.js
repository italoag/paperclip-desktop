// Bridge between the launcher UI and the Rust backend.
//
// This reproduces the exact `window.paperclipLauncher` surface that
// `src/launcher-preload.ts` exposed through Electron's contextBridge, so
// `launcher.js` needed no changes to its call sites during the migration.
//
// It relies on `app.withGlobalTauri` being enabled, which keeps the launcher a
// set of plain static files with no bundler or npm dependency — the smallest
// possible frontend for a window that only draws a connection chooser.
//
// Must load before `launcher.js`, which captures `window.paperclipLauncher`
// at its first line.

(() => {
  const tauri = window.__TAURI__;
  if (!tauri) {
    console.error("[bridge] window.__TAURI__ is unavailable; is withGlobalTauri enabled?");
    return;
  }

  const { invoke } = tauri.core;
  const { listen } = tauri.event;

  // Tauri's `listen` resolves to an unlisten function. The launcher subscribes
  // once for the lifetime of the window and never unsubscribes, so the promise
  // is intentionally dropped — but a rejection should still be visible.
  const on = (event, callback) => {
    listen(event, (message) => callback(message.payload)).catch((error) =>
      console.error(`[bridge] could not subscribe to ${event}:`, error),
    );
  };

  window.paperclipLauncher = {
    bootstrap: () => invoke("launcher_bootstrap"),
    setChooserMode: (mode) => invoke("launcher_set_chooser_mode", { mode }),
    saveRemoteProfile: (payload) => invoke("launcher_save_remote_profile", { payload }),
    duplicateProfile: (profileId) => invoke("launcher_duplicate_profile", { profileId }),
    deleteProfile: (profileId) => invoke("launcher_delete_profile", { profileId }),
    verifyRemote: (payload) => invoke("launcher_verify_remote", { payload }),
    connectLocal: (payload) => invoke("launcher_connect_local", { payload }),
    connectRemote: (payload) => invoke("launcher_connect_remote", { payload }),
    connectSavedProfile: (payload) => invoke("launcher_connect_saved_profile", { payload }),
    closeSheet: () => invoke("launcher_close_sheet"),
    showChooser: () => invoke("launcher_show_chooser"),
    reportContentHeight: (height) => invoke("launcher_report_content_height", { height }),

    onStateChanged: (callback) => on("launcher:state-changed", callback),
    onNavigate: (callback) => on("launcher:navigate", callback),
    onBootStatus: (callback) => on("launcher:boot-status", callback),
    onConnectionError: (callback) => on("launcher:connection-error", callback),
    onRequestContentHeightSync: (callback) =>
      on("launcher:request-content-height-sync", callback),
  };
})();
