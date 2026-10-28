//! Guard rails around the app's on-disk data roots.
//!
//! Direct port of `src/runtime-safety.ts`. The smoke tests run the packaged app
//! against throwaway data directories, and this module is what refuses to let
//! that accidentally point at the user's real Paperclip database.
//!
//! `applyDesktopUserDataOverride` has no counterpart here: Electron needed it to
//! call `app.setPath("userData", ...)` before startup, whereas the Tauri port
//! resolves the directory on demand in [`crate::paths`].

use std::path::{Path, PathBuf};

pub const DESKTOP_USER_DATA_DIR_ENV: &str = "PAPERCLIP_DESKTOP_USER_DATA_DIR";
pub const PAPERCLIP_HOME_ENV: &str = "PAPERCLIP_HOME";
pub const REQUIRE_ISOLATED_DATA_ENV: &str = "PAPERCLIP_DESKTOP_REQUIRE_ISOLATED_DATA";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuntimeSafetyError {
    #[error("{0} must be an absolute path.")]
    NotAbsolute(&'static str),

    #[error(
        "{REQUIRE_ISOLATED_DATA_ENV}=1 requires both {DESKTOP_USER_DATA_DIR_ENV} and \
         {PAPERCLIP_HOME_ENV} to be absolute paths."
    )]
    MissingIsolatedRoots,

    #[error("{REQUIRE_ISOLATED_DATA_ENV}=1 expected userData to be {expected}, got {actual}")]
    UserDataMismatch { expected: String, actual: String },

    #[error(
        "{REQUIRE_ISOLATED_DATA_ENV}=1 expected {PAPERCLIP_HOME_ENV} to be {expected}, got {actual}"
    )]
    PaperclipHomeMismatch { expected: String, actual: String },

    #[error("{REQUIRE_ISOLATED_DATA_ENV}=1 refused to use production Paperclip data path: {0}")]
    ProductionPathRefused(String),
}

/// The subset of the environment this module reads, captured up front so tests
/// do not have to mutate process-global state.
#[derive(Debug, Default, Clone)]
pub struct RuntimeEnv {
    pub desktop_user_data_dir: Option<String>,
    pub paperclip_home: Option<String>,
    pub require_isolated_data: Option<String>,
}

impl RuntimeEnv {
    pub fn from_process() -> Self {
        Self {
            desktop_user_data_dir: std::env::var(DESKTOP_USER_DATA_DIR_ENV).ok(),
            paperclip_home: std::env::var(PAPERCLIP_HOME_ENV).ok(),
            require_isolated_data: std::env::var(REQUIRE_ISOLATED_DATA_ENV).ok(),
        }
    }

    pub fn requires_isolated_data(&self) -> bool {
        self.require_isolated_data.as_deref() == Some("1")
    }

    pub fn user_data_override(&self) -> Result<Option<PathBuf>, RuntimeSafetyError> {
        read_absolute_path(
            self.desktop_user_data_dir.as_deref(),
            DESKTOP_USER_DATA_DIR_ENV,
        )
    }

    pub fn paperclip_home_override(&self) -> Result<Option<PathBuf>, RuntimeSafetyError> {
        read_absolute_path(self.paperclip_home.as_deref(), PAPERCLIP_HOME_ENV)
    }
}

/// Where the embedded server should keep its data.
///
/// Mirrors `resolvePaperclipHomePath`: an explicit override wins, otherwise the
/// legacy `~/.paperclip` directory is reused when it already holds a database
/// (so the desktop app and the CLI share one instance), and only then do we fall
/// back to the app's own data directory.
pub fn resolve_paperclip_home(
    env: &RuntimeEnv,
    home: &Path,
    user_data: &Path,
    path_exists: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, RuntimeSafetyError> {
    if let Some(override_path) = env.paperclip_home_override()? {
        return Ok(override_path);
    }

    let default_home = home.join(".paperclip");
    if !home.as_os_str().is_empty()
        && path_exists(&default_home.join("instances").join("default").join("db"))
    {
        return Ok(default_home);
    }

    Ok(user_data.to_path_buf())
}

/// Refuses to run against production data when isolation is demanded.
pub fn assert_isolated_runtime_paths(
    env: &RuntimeEnv,
    user_data: &Path,
    paperclip_home: &Path,
    home: &Path,
    platform: Platform,
) -> Result<(), RuntimeSafetyError> {
    if !env.requires_isolated_data() {
        return Ok(());
    }

    let (Some(user_data_override), Some(home_override)) =
        (env.user_data_override()?, env.paperclip_home_override()?)
    else {
        return Err(RuntimeSafetyError::MissingIsolatedRoots);
    };

    let normalized_user_data = normalize(user_data);
    let normalized_home = normalize(paperclip_home);

    if normalized_user_data != normalize(&user_data_override) {
        return Err(RuntimeSafetyError::UserDataMismatch {
            expected: user_data_override.display().to_string(),
            actual: user_data.display().to_string(),
        });
    }
    if normalized_home != normalize(&home_override) {
        return Err(RuntimeSafetyError::PaperclipHomeMismatch {
            expected: home_override.display().to_string(),
            actual: paperclip_home.display().to_string(),
        });
    }

    for production in production_paperclip_data_paths(platform, home) {
        let production = normalize(&production);
        if normalized_user_data == production || normalized_home == production {
            return Err(RuntimeSafetyError::ProductionPathRefused(
                production.display().to_string(),
            ));
        }
    }

    Ok(())
}

/// Data roots that must never be touched by an isolated run.
pub fn production_paperclip_data_paths(platform: Platform, home: &Path) -> Vec<PathBuf> {
    if home.as_os_str().is_empty() {
        return Vec::new();
    }

    let app_data = match platform {
        Platform::MacOs => home
            .join("Library")
            .join("Application Support")
            .join("Paperclip"),
        Platform::Windows => home.join("AppData").join("Roaming").join("Paperclip"),
        Platform::Other => home.join(".config").join("Paperclip"),
    };

    vec![app_data, home.join(".paperclip")]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Other
    };
}

fn read_absolute_path(
    value: Option<&str>,
    name: &'static str,
) -> Result<Option<PathBuf>, RuntimeSafetyError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(RuntimeSafetyError::NotAbsolute(name));
    }

    Ok(Some(normalize(path)))
}

/// Lexical normalisation only — `path::resolve` in Node does not touch the disk
/// either, and these paths may not exist yet.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(user_data: Option<&str>, home: Option<&str>, isolated: bool) -> RuntimeEnv {
        RuntimeEnv {
            desktop_user_data_dir: user_data.map(String::from),
            paperclip_home: home.map(String::from),
            require_isolated_data: isolated.then(|| "1".to_string()),
        }
    }

    #[test]
    fn runtime_path_overrides_must_be_absolute() {
        let relative = env(Some("relative/path"), None, false);
        assert_eq!(
            relative.user_data_override(),
            Err(RuntimeSafetyError::NotAbsolute(DESKTOP_USER_DATA_DIR_ENV))
        );

        let relative_home = env(None, Some("relative/path"), false);
        assert_eq!(
            resolve_paperclip_home(
                &relative_home,
                Path::new("/tmp/home"),
                Path::new("/tmp/user-data"),
                &|_| true
            ),
            Err(RuntimeSafetyError::NotAbsolute(PAPERCLIP_HOME_ENV))
        );
    }

    #[test]
    fn desktop_user_data_override_is_returned_verbatim() {
        let override_path = "/tmp/paperclip/user-data";
        let env = env(Some(override_path), None, false);
        assert_eq!(
            env.user_data_override().unwrap(),
            Some(PathBuf::from(override_path))
        );
    }

    #[test]
    fn paperclip_home_override_wins_over_legacy_detection() {
        let override_path = "/tmp/paperclip-home";
        let resolved = resolve_paperclip_home(
            &env(None, Some(override_path), false),
            Path::new("/Users/example"),
            Path::new("/Users/example/Library/Application Support/Paperclip"),
            &|_| true,
        )
        .unwrap();

        assert_eq!(resolved, PathBuf::from(override_path));
    }

    #[test]
    fn legacy_paperclip_home_fallback_is_unchanged() {
        let user_data = Path::new("/Users/example/Library/Application Support/Paperclip");

        let existing = resolve_paperclip_home(
            &RuntimeEnv::default(),
            Path::new("/Users/example"),
            user_data,
            &|target| target.ends_with(".paperclip/instances/default/db"),
        )
        .unwrap();
        let missing = resolve_paperclip_home(
            &RuntimeEnv::default(),
            Path::new("/Users/example"),
            user_data,
            &|_| false,
        )
        .unwrap();

        assert_eq!(existing, PathBuf::from("/Users/example/.paperclip"));
        assert_eq!(missing, user_data);
    }

    #[test]
    fn isolated_mode_requires_both_explicit_roots() {
        let result = assert_isolated_runtime_paths(
            &env(None, None, true),
            Path::new("/tmp/user-data"),
            Path::new("/tmp/paperclip-home"),
            Path::new("/Users/example"),
            Platform::MacOs,
        );

        assert_eq!(result, Err(RuntimeSafetyError::MissingIsolatedRoots));
    }

    #[test]
    fn isolated_mode_rejects_production_paths() {
        let home = Path::new("/Users/example");
        let production = production_paperclip_data_paths(Platform::MacOs, home)
            .into_iter()
            .next()
            .unwrap();
        let production = production.to_str().unwrap().to_string();

        let result = assert_isolated_runtime_paths(
            &env(Some(&production), Some("/tmp/paperclip-home"), true),
            Path::new(&production),
            Path::new("/tmp/paperclip-home"),
            home,
            Platform::MacOs,
        );

        assert_eq!(
            result,
            Err(RuntimeSafetyError::ProductionPathRefused(production))
        );
    }

    #[test]
    fn isolated_mode_requires_actual_paths_to_match_overrides() {
        let result = assert_isolated_runtime_paths(
            &env(
                Some("/tmp/paperclip-smoke/user-data"),
                Some("/tmp/paperclip-smoke/paperclip-home"),
                true,
            ),
            Path::new("/tmp/other-user-data"),
            Path::new("/tmp/paperclip-smoke/paperclip-home"),
            Path::new("/Users/example"),
            Platform::MacOs,
        );

        assert!(matches!(
            result,
            Err(RuntimeSafetyError::UserDataMismatch { .. })
        ));
    }

    #[test]
    fn isolated_mode_allows_explicit_temporary_roots() {
        let result = assert_isolated_runtime_paths(
            &env(
                Some("/tmp/paperclip-smoke/user-data"),
                Some("/tmp/paperclip-smoke/paperclip-home"),
                true,
            ),
            Path::new("/tmp/paperclip-smoke/user-data"),
            Path::new("/tmp/paperclip-smoke/paperclip-home"),
            Path::new("/Users/example"),
            Platform::MacOs,
        );

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn production_paths_are_empty_without_a_home() {
        assert!(production_paperclip_data_paths(Platform::MacOs, Path::new("")).is_empty());
    }
}
