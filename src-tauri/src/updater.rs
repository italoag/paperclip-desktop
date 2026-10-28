//! Auto-update, on top of `tauri-plugin-updater`.
//!
//! Port of `src/updater.ts`, which used `electron-updater`. The user-facing
//! behaviour is the same — check on launch and every four hours, download in
//! the background, then offer to restart — but the transport is different and
//! deliberately incompatible:
//!
//! | | Electron | Tauri |
//! |---|---|---|
//! | Manifest | `latest-mac.yml` | `latest.json` |
//! | Artifact | `.zip` | `.app.tar.gz` |
//! | Integrity | none beyond the code signature | minisign signature, verified before install |
//!
//! There is no bridge for clients still on the Electron build; that was an
//! explicit decision, so the first Tauri release is a fresh install.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// Guards against overlapping checks and stacked restart prompts.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Starts the periodic background check.
pub fn start<R: Runtime>(app: AppHandle<R>) {
    if cfg!(debug_assertions) {
        log::info!("[updater] skipping update checks in a debug build");
        return;
    }

    tauri::async_runtime::spawn(async move {
        loop {
            check(&app, Notify::OnlyWhenUpdated).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// "Check for Updates" from the application menu.
pub fn check_from_menu<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        check(&app, Notify::Always).await;
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notify {
    /// Silent unless there is something to install.
    OnlyWhenUpdated,
    /// Always report the outcome, including "you are up to date".
    Always,
}

async fn check<R: Runtime>(app: &AppHandle<R>, notify: Notify) {
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        if notify == Notify::Always {
            inform(
                app,
                "Check for Updates",
                "An update check is already running.",
            );
        }
        return;
    }
    let _guard = InFlightGuard;

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => {
            report_failure(app, notify, &error.to_string());
            return;
        }
    };

    match updater.check().await {
        Ok(Some(update)) => offer(app, update).await,
        Ok(None) => {
            if notify == Notify::Always {
                inform(
                    app,
                    "You're Up to Date",
                    format!(
                        "Paperclip v{} is the latest version.",
                        app.package_info().version
                    ),
                );
            }
        }
        Err(error) => report_failure(app, notify, &error.to_string()),
    }
}

async fn offer<R: Runtime>(app: &AppHandle<R>, update: tauri_plugin_updater::Update) {
    let version = update.version.clone();

    let accepted = app
        .dialog()
        .message(format!(
            "Paperclip v{version} is available. Download and install it now?"
        ))
        .title("Update Available")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Download".into(),
            "Later".into(),
        ))
        .blocking_show();

    if !accepted {
        return;
    }

    // The plugin verifies the minisign signature before writing anything.
    if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
        log::error!("[updater] install failed: {error}");
        app.dialog()
            .message(format!(
                "Paperclip could not install the update.\n\n{error}"
            ))
            .title("Update Failed")
            .kind(MessageDialogKind::Error)
            .blocking_show();
        return;
    }

    let restart = app
        .dialog()
        .message(format!(
            "Paperclip v{version} has been installed. Restart now to use it?"
        ))
        .title("Update Ready")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Restart".into(),
            "Later".into(),
        ))
        .blocking_show();

    if restart {
        // Give the embedded server a chance to shut down cleanly first.
        if let Some(state) = app.try_state::<Arc<crate::state::AppState>>() {
            state.set_quitting();
            crate::boot::stop_current_server(&state).await;
        }
        app.restart();
    }
}

fn report_failure<R: Runtime>(app: &AppHandle<R>, notify: Notify, detail: &str) {
    log::warn!("[updater] {detail}");
    if notify == Notify::Always {
        app.dialog()
            .message(format!(
                "Paperclip could not check for updates.\n\n{detail}"
            ))
            .title("Update Check Failed")
            .kind(MessageDialogKind::Error)
            .blocking_show();
    }
}

fn inform<R: Runtime>(app: &AppHandle<R>, title: &str, message: impl Into<String>) {
    app.dialog()
        .message(message.into())
        .title(title)
        .blocking_show();
}

struct InFlightGuard;

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}
