//! Rotation for the embedded server's `server.log`.
//!
//! Port of the log helpers in `src/main.ts`.
//!
//! # The quarantine step is a security control, not tidiness
//!
//! The log is opened in append mode. If anything replaced `server.log` with a
//! symlink, appending would write server output — which can contain secrets —
//! into whatever the link points at, and could clobber an unrelated file. So
//! before opening, anything that is not a *regular file* is moved aside under a
//! distinct prefix. `symlink_metadata` is used deliberately: `metadata` follows
//! links and would report the target as a regular file.

use std::path::{Path, PathBuf};

use uuid::Uuid;

pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
pub const ARCHIVE_KEEP: usize = 5;
pub const ARCHIVE_PREFIX: &str = "server.log.";
pub const UNSAFE_PREFIX: &str = "server.log.unsafe.";

const STAMP: &[time::format_description::FormatItem<'static>] = time::macros::format_description!(
    "[year][month][day]-[hour][minute][second]-[subsecond digits:3]Z"
);

pub fn log_file_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join("server.log")
}

fn archive_dir(log_file: &Path) -> PathBuf {
    log_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("log-archive")
}

fn archive_path(log_file: &Path, prefix: &str) -> PathBuf {
    let stamp = time::OffsetDateTime::now_utc()
        .format(STAMP)
        .unwrap_or_else(|_| "unknown".to_string());
    archive_dir(log_file).join(format!("{prefix}{stamp}.{}", Uuid::new_v4()))
}

/// Moves the current log into the archive directory.
fn archive(log_file: &Path, prefix: &str) -> std::io::Result<()> {
    let target = archive_path(log_file, prefix);
    std::fs::create_dir_all(archive_dir(log_file))?;
    std::fs::rename(log_file, target)
}

/// Moves aside anything at the log path that is not a plain file.
///
/// Best-effort by design: a failure here must not stop the server from booting,
/// which matches the original's empty `catch`.
pub fn quarantine_unsafe_log(log_file: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(log_file) else {
        return; // Nothing there yet, which is the normal case.
    };

    if metadata.file_type().is_file() {
        return;
    }

    let _ = archive(log_file, UNSAFE_PREFIX);
}

/// Rotates the log once it grows past [`MAX_BYTES`], then prunes old archives.
pub fn rotate_if_large(log_file: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(log_file) else {
        return;
    };

    if !metadata.file_type().is_file() || metadata.len() < MAX_BYTES {
        return;
    }

    if archive(log_file, ARCHIVE_PREFIX).is_ok() {
        prune_archives(log_file);
    }
}

/// Keeps only the newest [`ARCHIVE_KEEP`] rotations.
///
/// The timestamp prefix makes lexicographic order chronological, which is what
/// the original relied on too.
fn prune_archives(log_file: &Path) {
    let dir = archive_dir(log_file);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };

    let mut archives: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(ARCHIVE_PREFIX))
                && entry
                    .file_type()
                    .map(|file_type| file_type.is_file())
                    .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .collect();

    archives.sort();

    let excess = archives.len().saturating_sub(ARCHIVE_KEEP);
    for stale in archives.into_iter().take(excess) {
        let _ = std::fs::remove_file(stale);
    }
}

/// Prepares the log path and opens it for appending.
pub fn open_for_append(log_file: &Path) -> std::io::Result<std::fs::File> {
    quarantine_unsafe_log(log_file);
    rotate_if_large(log_file);

    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archived(log_file: &Path, prefix: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(archive_dir(log_file)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name.starts_with(prefix))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_symlinked_log_is_quarantined_instead_of_being_appended_to() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "original contents").unwrap();

        let log_file = log_file_path(dir.path());
        std::os::unix::fs::symlink(&secret, &log_file).unwrap();

        let mut file = open_for_append(&log_file).unwrap();
        use std::io::Write;
        writeln!(file, "server output that must not leak").unwrap();
        drop(file);

        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "original contents",
            "the symlink target must be untouched"
        );
        assert!(!std::fs::symlink_metadata(&log_file).unwrap().is_symlink());
        assert_eq!(archived(&log_file, UNSAFE_PREFIX).len(), 1);
    }

    #[test]
    fn a_directory_at_the_log_path_is_quarantined_too() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = log_file_path(dir.path());
        std::fs::create_dir(&log_file).unwrap();

        open_for_append(&log_file).unwrap();

        assert!(std::fs::symlink_metadata(&log_file).unwrap().is_file());
        assert_eq!(archived(&log_file, UNSAFE_PREFIX).len(), 1);
    }

    #[test]
    fn a_regular_log_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = log_file_path(dir.path());
        std::fs::write(&log_file, "existing\n").unwrap();

        open_for_append(&log_file).unwrap();

        assert_eq!(std::fs::read_to_string(&log_file).unwrap(), "existing\n");
        assert!(archived(&log_file, UNSAFE_PREFIX).is_empty());
    }

    #[test]
    fn appending_preserves_earlier_content() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = log_file_path(dir.path());

        for line in ["first", "second"] {
            use std::io::Write;
            let mut file = open_for_append(&log_file).unwrap();
            writeln!(file, "{line}").unwrap();
        }

        assert_eq!(
            std::fs::read_to_string(&log_file).unwrap(),
            "first\nsecond\n"
        );
    }

    #[test]
    fn an_oversized_log_is_rotated_away() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = log_file_path(dir.path());
        std::fs::write(&log_file, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();

        open_for_append(&log_file).unwrap();

        assert_eq!(std::fs::metadata(&log_file).unwrap().len(), 0);
        assert_eq!(archived(&log_file, ARCHIVE_PREFIX).len(), 1);
    }

    #[test]
    fn a_log_just_under_the_limit_is_not_rotated() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = log_file_path(dir.path());
        std::fs::write(&log_file, vec![b'x'; (MAX_BYTES - 1) as usize]).unwrap();

        open_for_append(&log_file).unwrap();

        assert_eq!(
            std::fs::metadata(&log_file).unwrap().len(),
            MAX_BYTES - 1,
            "must not rotate below the threshold"
        );
        assert!(archived(&log_file, ARCHIVE_PREFIX).is_empty());
    }

    #[test]
    fn only_the_newest_archives_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = log_file_path(dir.path());
        let archive = archive_dir(&log_file);
        std::fs::create_dir_all(&archive).unwrap();

        // Pre-seed more rotations than the retention limit, plus an unrelated
        // file that must survive.
        for index in 0..ARCHIVE_KEEP + 3 {
            std::fs::write(
                archive.join(format!("{ARCHIVE_PREFIX}2026010{index}-000000-000Z.x")),
                "old",
            )
            .unwrap();
        }
        std::fs::write(archive.join("unrelated.txt"), "keep me").unwrap();

        std::fs::write(&log_file, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        open_for_append(&log_file).unwrap();

        let remaining = archived(&log_file, ARCHIVE_PREFIX);
        assert_eq!(remaining.len(), ARCHIVE_KEEP);
        // The oldest went first.
        assert!(!remaining.iter().any(|name| name.contains("20260100")));
        assert!(archive.join("unrelated.txt").exists());
    }
}
