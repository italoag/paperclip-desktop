//! Locating a Node runtime and rebuilding a usable `PATH` for the server.
//!
//! Port of `findNodeBinary` and `resolveShellPath` in `src/main.ts`.
//!
//! # Why PATH needs rebuilding at all
//!
//! An app launched from Finder or the Dock inherits `launchd`'s minimal
//! environment, not the user's shell `PATH`. The Paperclip server shells out to
//! developer tools (the `claude` CLI and friends), so without this they simply
//! are not found — and the failure looks like a Paperclip bug rather than an
//! environment one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Directories worth having on `PATH` even when the login shell omits them.
const SYSTEM_FALLBACKS: [&str; 7] = [
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// Well-known Node install locations, in the order the original checked them.
const NODE_FALLBACKS: [&str; 3] = [
    "/usr/local/bin/node",
    "/opt/homebrew/bin/node",
    "/usr/bin/node",
];

/// The login shell probe is a subprocess; never let it stall startup.
const SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Picks the Node binary used to run the server.
///
/// The bundled runtime always wins so a packaged install never depends on what
/// the user happens to have installed.
pub fn find_node_binary(app_root: Option<&Path>, home: Option<&Path>) -> PathBuf {
    let Some(app_root) = app_root else {
        // Development: whatever `node` the developer's PATH resolves.
        return PathBuf::from("node");
    };

    let bundled = app_root
        .join("node-bin")
        .join(if cfg!(windows) { "node.exe" } else { "node" });
    if is_executable(&bundled) {
        return bundled;
    }

    log::warn!("bundled Node runtime missing at {}", bundled.display());

    let mut candidates = Vec::new();
    if let Some(version_bin) = home.and_then(nvm_default_bin_dir) {
        candidates.push(version_bin.join("node"));
    }
    candidates.extend(NODE_FALLBACKS.iter().map(PathBuf::from));

    candidates
        .into_iter()
        .find(|candidate| is_executable(candidate))
        .unwrap_or_else(|| PathBuf::from("node"))
}

/// Builds the `PATH` handed to the server process.
pub fn resolve_shell_path(home: Option<&Path>, current_path: Option<&str>) -> String {
    let base = probe_login_shell_path()
        .or_else(|| current_path.map(str::to_string))
        .unwrap_or_default();

    merge_path(&base, &fallback_dirs(home))
}

/// Directories to guarantee on `PATH`, most specific first.
pub fn fallback_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(home) = home {
        if let Some(version_bin) = nvm_default_bin_dir(home) {
            dirs.push(version_bin);
        }
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".npm-global").join("bin"));
    }

    dirs.extend(SYSTEM_FALLBACKS.iter().map(PathBuf::from));
    dirs
}

/// Appends any missing fallback directory, preserving the base order.
///
/// The user's own `PATH` keeps priority: fallbacks are only ever appended, never
/// prepended, so a project-local toolchain still shadows a system one.
pub fn merge_path(base: &str, fallbacks: &[PathBuf]) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };

    let existing: HashSet<&str> = base.split(separator).collect();
    let missing: Vec<String> = fallbacks
        .iter()
        .filter_map(|dir| dir.to_str())
        .filter(|dir| !existing.contains(dir))
        // A fallback listed twice must only be appended once.
        .fold(Vec::new(), |mut acc, dir| {
            if !acc.iter().any(|seen| seen == dir) {
                acc.push(dir.to_string());
            }
            acc
        });

    if missing.is_empty() {
        return base.to_string();
    }
    if base.is_empty() {
        return missing.join(&separator.to_string());
    }

    format!("{base}{separator}{}", missing.join(&separator.to_string()))
}

fn nvm_default_bin_dir(home: &Path) -> Option<PathBuf> {
    let nvm_dir = std::env::var_os("NVM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".nvm"));

    let version = std::fs::read_to_string(nvm_dir.join("alias").join("default")).ok()?;
    let version = version.trim();
    if version.is_empty() {
        return None;
    }

    Some(
        nvm_dir
            .join("versions")
            .join("node")
            .join(version)
            .join("bin"),
    )
}

/// Asks the user's login shell what its `PATH` is.
fn probe_login_shell_path() -> Option<String> {
    if cfg!(windows) {
        return None;
    }

    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| Path::new(shell).is_absolute())
        .unwrap_or_else(|| "/bin/zsh".to_string());

    // A login shell sources the user's profile, which is the whole point, but it
    // can also hang on a misbehaving rc file — hence the timeout.
    let output = std::process::Command::new(shell)
        .args(["-lc", "echo $PATH"])
        .stdin(std::process::Stdio::null())
        // Both pipes must be captured. Leaving stdout inherited would dump the
        // user's whole PATH into our own output *and* leave nothing for
        // `wait_with_output` to read, silently defeating the probe.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
        .and_then(|child| wait_with_timeout(child, SHELL_PROBE_TIMEOUT))?;

    let path = String::from_utf8_lossy(&output).trim().to_string();
    (!path.is_empty()).then_some(path)
}

fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> Option<Vec<u8>> {
    let deadline = std::time::Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Err(_) => return None,
            Ok(None) => {}
        }

        if std::time::Instant::now() >= deadline {
            log::warn!("login shell PATH probe timed out");
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    child.wait_with_output().ok().map(|output| output.stdout)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_executable(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn merge_path_appends_only_what_is_missing() {
        let merged = merge_path(
            "/opt/homebrew/bin:/usr/bin",
            &[
                PathBuf::from("/usr/bin"),
                PathBuf::from("/sbin"),
                PathBuf::from("/opt/homebrew/bin"),
            ],
        );

        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/sbin");
    }

    #[test]
    fn merge_path_keeps_the_users_ordering() {
        let merged = merge_path("/my/tools:/usr/bin", &[PathBuf::from("/usr/local/bin")]);
        assert!(
            merged.starts_with("/my/tools:/usr/bin"),
            "user PATH must keep priority: {merged}"
        );
    }

    #[test]
    fn merge_path_handles_an_empty_base_and_duplicate_fallbacks() {
        assert_eq!(
            merge_path("", &[PathBuf::from("/bin"), PathBuf::from("/bin")]),
            "/bin"
        );
        assert_eq!(merge_path("/bin", &[]), "/bin");
    }

    #[test]
    fn fallback_dirs_prefer_user_locations_over_system_ones() {
        let home = PathBuf::from("/Users/example");
        let dirs = fallback_dirs(Some(&home));

        let local_bin = dirs
            .iter()
            .position(|dir| dir == &home.join(".local").join("bin"))
            .expect("~/.local/bin should be present");
        let usr_bin = dirs
            .iter()
            .position(|dir| dir == Path::new("/usr/bin"))
            .expect("/usr/bin should be present");

        assert!(local_bin < usr_bin);
    }

    #[test]
    fn the_bundled_runtime_wins_over_anything_installed() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("node-bin").join("node");
        touch_executable(&bundled);

        assert_eq!(find_node_binary(Some(dir.path()), None), bundled);
    }

    #[test]
    fn a_non_executable_bundled_runtime_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("node-bin").join("node");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, "not executable").unwrap();

        assert_ne!(find_node_binary(Some(dir.path()), None), bundled);
    }

    #[test]
    fn development_falls_back_to_path_lookup() {
        assert_eq!(find_node_binary(None, None), PathBuf::from("node"));
    }

    #[test]
    fn a_missing_bundle_falls_back_to_a_known_location_or_plain_node() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = find_node_binary(Some(dir.path()), None);

        assert!(
            resolved == Path::new("node")
                || NODE_FALLBACKS
                    .iter()
                    .any(|known| resolved == Path::new(known)),
            "unexpected fallback: {}",
            resolved.display()
        );
    }
}
