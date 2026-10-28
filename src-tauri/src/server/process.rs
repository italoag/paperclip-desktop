//! Spawning and reliably tearing down the embedded server process tree.
//!
//! Replaces the `tree-kill` npm package used by `src/main.ts`.
//!
//! # Why a process group instead of walking children
//!
//! `tree-kill` shells out to `ps` to discover descendants and signals them one
//! by one, which races against a tree that is still spawning. Here the child is
//! made a *process group leader* at spawn time, so a single `killpg` reaches the
//! server, the PostgreSQL it started, and anything else it forked — atomically
//! and with no shelling out.
//!
//! # Escalation
//!
//! The original signalled `SIGTERM` and immediately resolved, so quitting the
//! app could race PostgreSQL's shutdown flush. Here `SIGTERM` is followed by a
//! grace period and then `SIGKILL`, which is both safer for the database and
//! guarantees no orphan survives.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};

/// How long a polite shutdown gets before the group is killed outright.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("could not start the Paperclip server: {0}")]
    Spawn(#[from] std::io::Error),
}

/// Builds the server command with the child isolated into its own process group.
pub fn command(program: &Path, args: &[&Path], cwd: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Do not leave a stray process behind if we are killed abruptly.
        .kill_on_drop(false);

    #[cfg(unix)]
    {
        // pgid == pid, which is what `kill_tree` signals.
        command.process_group(0);
    }

    command
}

/// Sends `SIGTERM` to the whole group, escalating to `SIGKILL` after the grace
/// period. Returns `true` if the process exited before escalation.
pub async fn shutdown(child: &mut Child, grace: Duration) -> bool {
    let Some(pid) = child.id() else {
        // Already reaped.
        return true;
    };

    signal_tree(pid, Signal::Term);

    let deadline = tokio::time::Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Err(_) => return true, // Nothing left to signal.
            Ok(None) => {}
        }

        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    log::warn!("server ignored SIGTERM after {grace:?}; sending SIGKILL");
    signal_tree(pid, Signal::Kill);
    let _ = child.wait().await;
    false
}

#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Term,
    Kill,
}

/// Signals a process and, when it leads one, its whole process group.
///
/// Both are attempted on purpose. `killpg` alone is not enough: it only reaches
/// a group whose id equals `pid`, which holds for servers *we* spawned (they are
/// made leaders in [`command`]) but not necessarily for a pid recovered from a
/// stale pid file. Sending to the pid as well guarantees the process itself dies
/// even when it leads no group. Signalling twice is harmless — the second call
/// simply finds nothing left.
pub fn signal_tree(pid: u32, signal: Signal) {
    #[cfg(unix)]
    {
        let signal = match signal {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // SAFETY: plain libc calls; a missing target is reported through errno,
        // which we ignore because it means the work is already done.
        unsafe {
            libc::killpg(pid as libc::pid_t, signal);
            libc::kill(pid as libc::pid_t, signal);
        }
    }

    #[cfg(windows)]
    {
        let flag = match signal {
            Signal::Term => "/T",
            Signal::Kill => "/F",
        };
        let _ = std::process::Command::new("taskkill")
            .args([flag, "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Whether a process with this pid currently exists.
///
/// Note that a *zombie* — a child that has exited but has not been waited on —
/// still counts as existing. That is fine for the only production caller
/// ([`super::pid_file::kill_orphaned_server`]), whose target is a process from a
/// previous app run and therefore never our child.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 performs the permission/existence check without delivering.
        // SAFETY: plain libc call, no pointers involved.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Best-effort command line for a pid, used to confirm identity before killing.
pub fn command_line(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!line.is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_support::signal_mutex;

    fn sleeper(seconds: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &format!("sleep {seconds}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        command
    }

    #[tokio::test]
    async fn is_alive_tracks_the_process_lifetime() {
        let _guard = signal_mutex().lock().await;
        let mut child = sleeper("30").spawn().unwrap();
        let pid = child.id().unwrap();

        assert!(is_alive(pid));
        shutdown(&mut child, SHUTDOWN_GRACE).await;
        assert!(!is_alive(pid));
    }

    #[tokio::test]
    async fn shutdown_reports_a_graceful_exit() {
        let _guard = signal_mutex().lock().await;
        let mut child = sleeper("30").spawn().unwrap();
        assert!(
            shutdown(&mut child, SHUTDOWN_GRACE).await,
            "sleep should die on SIGTERM without escalation"
        );
    }

    /// The whole point of the process group: descendants must die too, even when
    /// the direct child is only their parent shell.
    #[tokio::test]
    async fn shutdown_reaps_the_entire_process_tree() {
        let _guard = signal_mutex().lock().await;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("grandchild.pid");

        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                &format!("sleep 30 & echo $! > {}; wait", marker.display()),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().unwrap();

        // Wait for the grandchild pid to be recorded.
        let grandchild = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&marker) {
                    if let Ok(pid) = contents.trim().parse::<u32>() {
                        if is_alive(pid) {
                            return pid;
                        }
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .expect("grandchild never started");

        shutdown(&mut child, SHUTDOWN_GRACE).await;

        // Give the kernel a moment to reap.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !is_alive(grandchild),
            "grandchild {grandchild} outlived the process group"
        );
    }

    /// A server that traps SIGTERM must still be forced down rather than
    /// blocking app quit forever.
    #[tokio::test]
    async fn shutdown_escalates_to_sigkill_when_sigterm_is_ignored() {
        let _guard = signal_mutex().lock().await;
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("trap-installed");

        // Two details matter here:
        //  * the loop — a plain `trap '' TERM; sleep 30` would still exit,
        //    because the signal reaches the `sleep` child even though the shell
        //    ignores it, so the shell has to keep re-spawning;
        //  * the marker — signalling before the shell has run `trap` would hit
        //    the default disposition and kill it, making the test pass or fail
        //    depending on machine load.
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                &format!(
                    "trap '' TERM; : > {}; while :; do sleep 0.05; done",
                    ready.display()
                ),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the trap was never installed");

        let graceful = shutdown(&mut child, Duration::from_millis(300)).await;

        assert!(!graceful, "should report that escalation was needed");
        // `shutdown` awaits the child after SIGKILL, so it is already reaped and
        // the pid cannot be lingering as a zombie.
        assert!(!is_alive(pid));
    }

    #[tokio::test]
    async fn command_line_identifies_a_running_process() {
        let _guard = signal_mutex().lock().await;
        let mut child = sleeper("30").spawn().unwrap();
        let pid = child.id().unwrap();

        let line = command_line(pid).expect("ps should report the command");
        assert!(line.contains("sleep"), "unexpected command line: {line}");

        shutdown(&mut child, SHUTDOWN_GRACE).await;
    }

    #[test]
    fn command_line_is_none_for_a_dead_pid() {
        // Very unlikely to exist and not ours to signal either way.
        assert_eq!(command_line(u32::MAX - 1), None);
    }

    #[tokio::test]
    async fn the_built_command_captures_output_and_sets_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let output = command(Path::new("/bin/pwd"), &[], dir.path())
            .output()
            .await
            .unwrap();

        let printed = String::from_utf8_lossy(&output.stdout);
        // macOS reports /private/var for /var, so compare canonical paths.
        assert_eq!(
            std::fs::canonicalize(printed.trim()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }
}
