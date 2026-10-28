//! On-disk locations used by the shell.
//!
//! # Why this is not `app.path().app_data_dir()`
//!
//! Electron resolved `userData` from the *application name*, which `main.ts` set
//! to `"Paperclip"` via `app.setName("Paperclip")`, giving
//! `~/Library/Application Support/Paperclip` on macOS.
//!
//! Tauri instead derives its data directory from the **bundle identifier**,
//! which would give `~/Library/Application Support/com.paperclipai.app`. Using
//! that would silently orphan every existing install: saved remote profiles
//! (`connections.json`) would vanish and — far worse — the `PAPERCLIP_HOME`
//! fallback would hand the embedded server a brand new, empty database.
//!
//! So the legacy layout is reproduced verbatim here. Do not "simplify" this to
//! the Tauri helper.

use std::path::{Path, PathBuf};

use crate::runtime_safety::{Platform, RuntimeEnv, RuntimeSafetyError};

/// The product name Electron used for the data directory.
const LEGACY_APP_NAME: &str = "Paperclip";

/// Electron's `app.getPath("userData")` for a given platform and home directory.
pub fn legacy_user_data_dir(platform: Platform, home: &Path) -> PathBuf {
    match platform {
        Platform::MacOs => home
            .join("Library")
            .join("Application Support")
            .join(LEGACY_APP_NAME),
        Platform::Windows => home.join("AppData").join("Roaming").join(LEGACY_APP_NAME),
        Platform::Other => home.join(".config").join(LEGACY_APP_NAME),
    }
}

/// Resolves the data directory, honouring the smoke-test override.
pub fn user_data_dir(
    env: &RuntimeEnv,
    platform: Platform,
    home: &Path,
) -> Result<PathBuf, RuntimeSafetyError> {
    if let Some(override_path) = env.user_data_override()? {
        return Ok(override_path);
    }

    Ok(legacy_user_data_dir(platform, home))
}

/// Root of the bundled Paperclip server payload.
///
/// Packaged, this is `Contents/Resources/app-server` (see `bundle.macOS.files`);
/// in development it is the repository root, where the server is reachable at
/// `node_modules/@paperclipai/server`.
pub fn app_root(resource_dir: Option<&Path>) -> PathBuf {
    match resource_dir {
        Some(resources) => resources.join("app-server"),
        None => repo_root(),
    }
}

fn repo_root() -> PathBuf {
    // `src-tauri/..`
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_user_data_matches_the_electron_layout() {
        assert_eq!(
            legacy_user_data_dir(Platform::MacOs, Path::new("/Users/example")),
            PathBuf::from("/Users/example/Library/Application Support/Paperclip")
        );
    }

    #[test]
    fn windows_and_linux_match_the_electron_layout() {
        assert_eq!(
            legacy_user_data_dir(Platform::Windows, Path::new("/Users/example")),
            PathBuf::from("/Users/example/AppData/Roaming/Paperclip")
        );
        assert_eq!(
            legacy_user_data_dir(Platform::Other, Path::new("/home/example")),
            PathBuf::from("/home/example/.config/Paperclip")
        );
    }

    /// The isolation guard compares against these exact strings, so the two
    /// modules must agree or smoke runs would silently touch production data.
    #[test]
    fn user_data_dir_agrees_with_the_production_path_guard() {
        let home = Path::new("/Users/example");
        for platform in [Platform::MacOs, Platform::Windows, Platform::Other] {
            let guarded = crate::runtime_safety::production_paperclip_data_paths(platform, home);
            assert_eq!(
                guarded.first(),
                Some(&legacy_user_data_dir(platform, home)),
                "mismatch for {platform:?}"
            );
        }
    }

    #[test]
    fn override_wins_over_the_legacy_layout() {
        let env = RuntimeEnv {
            desktop_user_data_dir: Some("/tmp/smoke/user-data".into()),
            ..RuntimeEnv::default()
        };

        assert_eq!(
            user_data_dir(&env, Platform::MacOs, Path::new("/Users/example")).unwrap(),
            PathBuf::from("/tmp/smoke/user-data")
        );
    }

    #[test]
    fn app_root_points_at_the_bundled_payload_when_packaged() {
        assert_eq!(
            app_root(Some(Path::new("/Apps/Paperclip.app/Contents/Resources"))),
            PathBuf::from("/Apps/Paperclip.app/Contents/Resources/app-server")
        );
    }

    #[test]
    fn app_root_falls_back_to_the_repository_in_development() {
        let root = app_root(None);
        assert!(
            root.join("package.json").exists(),
            "expected the repo root, got {}",
            root.display()
        );
    }
}
