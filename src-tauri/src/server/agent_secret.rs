//! The signing secret the server needs to mint agent tokens.
//!
//! # Why the shell owns this
//!
//! `jwtConfig()` in the server reads `PAPERCLIP_AGENT_JWT_SECRET` (falling back
//! to `BETTER_AUTH_SECRET`) and returns `null` when neither is set. A null
//! config means no `PAPERCLIP_API_KEY` is ever minted, so every agent run gets
//! `401 Agent authentication required` on any control-plane write — hiring,
//! issues, comments, documents. The failure is silent from the server's side:
//! it boots and serves the UI perfectly well.
//!
//! Nothing else can supply it. A server deployment gets the secret from its
//! environment or `.env`; a desktop app has neither, and the user should not
//! have to invent one. So the shell generates it on first run and passes it in.
//!
//! # Properties this has to hold
//!
//! - **Stable across restarts.** The secret is folded into the per-company key
//!   derivation, so rotating it invalidates every outstanding token and, since
//!   better-auth accepts the same value, would drop user sessions too.
//! - **Per install.** A constant baked into the binary would be public the
//!   moment anyone unpacks the app, and every install would share it.
//! - **Not logged.** `ServerConfig` derives `Debug`; see `AgentJwtSecret`.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// File holding the secret, inside the app's user data directory.
const SECRET_FILE_NAME: &str = "agent-jwt-secret";

/// Owner-only. The directory already holds the database, so this does not widen
/// the trust boundary — but a world-readable signing key would.
#[cfg(unix)]
const SECRET_FILE_MODE: u32 = 0o600;

/// A secret that will not print itself.
///
/// `ServerConfig` derives `Debug`, so a bare `String` here would land in any
/// future `{:?}` — a crash report or a debug log line is all it would take.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentJwtSecret(String);

impl AgentJwtSecret {
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// A secret for this process only, used when the file cannot be persisted.
    ///
    /// Preferred over giving up: tokens are minted per run with a one-hour TTL,
    /// so an in-memory secret keeps agents working for the session and only
    /// costs validity across a restart. The alternative is a control plane that
    /// rejects every agent write.
    pub fn ephemeral() -> Self {
        Self(generate())
    }
}

impl fmt::Debug for AgentJwtSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentJwtSecret(<redacted>)")
    }
}

pub fn secret_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(SECRET_FILE_NAME)
}

/// 244 bits from two v4 UUIDs.
///
/// `uuid` is already a dependency and is CSPRNG-backed, which beats adding a
/// crate for this. HMAC-SHA256 takes any length; this is well past what it
/// needs.
fn generate() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // Created with the mode already set, rather than written then chmod'ed:
    // between those two steps the secret would sit on disk world-readable.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(SECRET_FILE_MODE)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    fs::write(path, contents)
}

/// Reads the stored secret, generating and persisting one on first run.
///
/// A file that exists but is empty or whitespace is treated as absent: an
/// interrupted first write would otherwise leave the app permanently unable to
/// mint tokens, with no way out but manual deletion.
pub fn resolve_or_create(user_data_dir: &Path) -> io::Result<AgentJwtSecret> {
    let path = secret_path(user_data_dir);

    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(AgentJwtSecret(trimmed.to_string()));
        }
    }

    fs::create_dir_all(user_data_dir)?;
    let secret = generate();
    write_private(&path, &secret)?;
    Ok(AgentJwtSecret(secret))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generates_a_secret_on_first_run() {
        let dir = tempdir().unwrap();
        let secret = resolve_or_create(dir.path()).unwrap();

        assert_eq!(secret.expose().len(), 64);
        assert!(secret_path(dir.path()).exists());
    }

    /// Rotating the secret invalidates every outstanding token, so the same
    /// install must keep reading the same value.
    #[test]
    fn returns_the_same_secret_on_later_runs() {
        let dir = tempdir().unwrap();
        let first = resolve_or_create(dir.path()).unwrap();
        let second = resolve_or_create(dir.path()).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn two_installs_do_not_share_a_secret() {
        let one = tempdir().unwrap();
        let two = tempdir().unwrap();

        assert_ne!(
            resolve_or_create(one.path()).unwrap(),
            resolve_or_create(two.path()).unwrap()
        );
    }

    /// An interrupted first write must not brick token minting forever.
    #[test]
    fn replaces_an_empty_secret_file() {
        let dir = tempdir().unwrap();
        fs::write(secret_path(dir.path()), "   \n").unwrap();

        let secret = resolve_or_create(dir.path()).unwrap();
        assert_eq!(secret.expose().len(), 64);
    }

    #[test]
    fn tolerates_a_trailing_newline_from_manual_editing() {
        let dir = tempdir().unwrap();
        fs::write(secret_path(dir.path()), "handwritten-secret\n").unwrap();

        assert_eq!(
            resolve_or_create(dir.path()).unwrap().expose(),
            "handwritten-secret"
        );
    }

    #[test]
    fn creates_the_data_directory_when_missing() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("does-not-exist-yet");

        assert!(resolve_or_create(&nested).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn stores_the_secret_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        resolve_or_create(dir.path()).unwrap();

        let mode = fs::metadata(secret_path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, SECRET_FILE_MODE);
    }

    /// The whole reason for the newtype: `ServerConfig` derives `Debug`.
    #[test]
    fn debug_output_hides_the_secret() {
        let secret = AgentJwtSecret("super-secret-value".to_string());

        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("super-secret-value"));
        assert!(rendered.contains("redacted"));
    }
}
