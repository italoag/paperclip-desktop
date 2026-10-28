//! Crash recovery for a server left running by a previous launch.
//!
//! Port of the pid-file handling in `src/main.ts`.
//!
//! # Hardening over the original
//!
//! The Electron version read the pid, checked that *some* process with that id
//! existed, and killed its whole tree. Pids are recycled, so after a crash and a
//! reboot that pid could belong to anything — and the app would terminate an
//! unrelated program's process group.
//!
//! The identity of the process is now confirmed before signalling: it must still
//! look like a Node process running the Paperclip server. Files written by the
//! Electron build (a bare integer) are still understood, so an upgrade still
//! cleans up after itself.

use std::path::{Path, PathBuf};

use super::process::{self, Signal};

pub const PID_FILE_NAME: &str = "paperclip-electron.pid";

/// Substrings that must appear in the process's command line for it to be
/// considered ours.
const IDENTITY_MARKERS: [&str; 2] = ["node", "index.js"];

pub fn pid_file_path(user_data_dir: &Path) -> PathBuf {
    user_data_dir.join(PID_FILE_NAME)
}

pub fn write(path: &Path, pid: u32) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, pid.to_string());
}

pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

pub fn read(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaped {
    /// No pid file, or it did not name a live process.
    Nothing,
    /// A live process was found but did not look like our server.
    SkippedForeign,
    Killed,
}

/// Terminates a server left behind by a previous run.
pub fn kill_orphaned_server(path: &Path) -> Reaped {
    let outcome = match read(path) {
        None => Reaped::Nothing,
        Some(pid) if !process::is_alive(pid) => Reaped::Nothing,
        Some(pid) => {
            if looks_like_our_server(pid) {
                log::info!("killing orphaned server process (pid={pid})");
                process::signal_tree(pid, Signal::Term);
                Reaped::Killed
            } else {
                log::warn!(
                    "pid {pid} from a stale pid file is not a Paperclip server; leaving it alone"
                );
                Reaped::SkippedForeign
            }
        }
    };

    clear(path);
    outcome
}

fn looks_like_our_server(pid: u32) -> bool {
    let Some(command_line) = process::command_line(pid) else {
        // Cannot confirm identity, so do not signal.
        return false;
    };

    let lowered = command_line.to_lowercase();
    IDENTITY_MARKERS
        .iter()
        .all(|marker| lowered.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_support::signal_lock;
    use std::process::{Command, Stdio};

    /// Spawns a test child as its own process-group leader.
    ///
    /// Matters for isolation: a child left in the harness's group could be
    /// reached by a `killpg` from another test, and it also mirrors production,
    /// where the server is always a group leader.
    fn spawn_isolated(script: &str) -> std::process::Child {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command.spawn().unwrap()
    }

    #[test]
    fn round_trips_a_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = pid_file_path(dir.path());

        write(&path, 4242);
        assert_eq!(read(&path), Some(4242));

        clear(&path);
        assert_eq!(read(&path), None);
    }

    /// The Electron build wrote a bare integer; keep reading those.
    #[test]
    fn reads_the_legacy_bare_integer_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = pid_file_path(dir.path());
        std::fs::write(&path, "  1234\n").unwrap();

        assert_eq!(read(&path), Some(1234));
    }

    #[test]
    fn rejects_garbage_and_nonsense_pids() {
        let dir = tempfile::tempdir().unwrap();
        let path = pid_file_path(dir.path());

        for contents in ["", "not a pid", "0", "-1"] {
            std::fs::write(&path, contents).unwrap();
            assert_eq!(read(&path), None, "accepted {contents:?}");
        }
    }

    #[test]
    fn a_missing_pid_file_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = pid_file_path(dir.path());

        assert_eq!(kill_orphaned_server(&path), Reaped::Nothing);
    }

    #[test]
    fn a_dead_pid_is_cleaned_up_without_signalling() {
        let _guard = signal_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = pid_file_path(dir.path());
        // Reserved by convention and never a live user process.
        write(&path, u32::MAX - 1);

        assert_eq!(kill_orphaned_server(&path), Reaped::Nothing);
        assert!(!path.exists(), "stale pid file must be removed");
    }

    /// The behaviour the Electron version got wrong: a recycled pid belonging to
    /// an unrelated program must not be killed.
    #[test]
    fn a_live_but_foreign_process_is_left_alone() {
        let _guard = signal_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = pid_file_path(dir.path());

        let mut victim = spawn_isolated("sleep 30");
        let pid = victim.id();
        write(&path, pid);

        assert_eq!(kill_orphaned_server(&path), Reaped::SkippedForeign);
        assert!(
            process::is_alive(pid),
            "an unrelated process must survive a stale pid file"
        );

        let _ = victim.kill();
        let _ = victim.wait();
    }

    #[test]
    fn a_process_that_looks_like_our_server_is_reaped() {
        let _guard = signal_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = pid_file_path(dir.path());

        // Stand-in whose command line carries both identity markers.
        let fake_server = dir.path().join("index.js");
        std::fs::write(&fake_server, "").unwrap();
        let mut child = spawn_isolated(&format!(
            "exec -a \"node {}\" sleep 30",
            fake_server.display()
        ));
        let pid = child.id();

        // Give the shell a moment to exec into the renamed process.
        std::thread::sleep(std::time::Duration::from_millis(300));

        let outcome = kill_orphaned_server(&path.with_file_name("other.pid"));
        assert_eq!(outcome, Reaped::Nothing, "no pid file yet");

        write(&path, pid);
        assert_eq!(kill_orphaned_server(&path), Reaped::Killed);

        // `is_alive` cannot be used here: this child belongs to the test process,
        // so between being signalled and being reaped it is a zombie, and
        // `kill(pid, 0)` still succeeds for zombies. Waiting on it is both the
        // reap and the assertion. In production the orphan comes from a previous
        // app run and is never our child, so no zombie can appear.
        let status = child.wait().unwrap();
        assert!(
            !status.success(),
            "expected termination by signal, got {status:?}"
        );
        assert!(!path.exists(), "the stale pid file must be removed");
    }
}
