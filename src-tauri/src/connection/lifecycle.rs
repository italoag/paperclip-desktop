//! Predicates for reasoning about overlapping server processes.
//!
//! Port of `src/connection/local-server-lifecycle.ts`.
//!
//! Booting local while a local server is already running means two child
//! processes exist at once. These decide which one wins, which one to reap, and
//! whether an exit event belongs to the process we are still tracking — the
//! difference between a clean restart and either killing the live server or
//! leaking an orphan.
//!
//! `ProcessLike` collapses to `Option<u32>` here; a missing or zero pid means
//! "no process", exactly like the `pid > 0` check in the original.

fn pid_of(process: Option<u32>) -> Option<u32> {
    process.filter(|pid| *pid > 0)
}

pub fn is_same_process(left: Option<u32>, right: Option<u32>) -> bool {
    match (pid_of(left), pid_of(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// An exit event only matters if it came from the process we still track.
pub fn should_handle_tracked_server_exit(tracked: Option<u32>, exited: Option<u32>) -> bool {
    is_same_process(tracked, exited)
}

/// After a successful reboot, the process that was replaced must be reaped.
pub fn should_kill_superseded_server(previous: Option<u32>, next: Option<u32>) -> bool {
    pid_of(previous).is_some() && pid_of(next).is_some() && !is_same_process(previous, next)
}

/// A boot that lost a race puts the previously tracked process back.
pub fn should_restore_previous_tracked_server(
    previous: Option<u32>,
    attempted: Option<u32>,
    tracked: Option<u32>,
) -> bool {
    pid_of(previous).is_some()
        && is_same_process(tracked, attempted)
        && !is_same_process(previous, attempted)
}

/// Only stop the process this boot started, and only while it is still tracked.
pub fn should_stop_attempted_server(attempted: Option<u32>, tracked: Option<u32>) -> bool {
    is_same_process(tracked, attempted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_zero_pids_never_match() {
        assert!(!is_same_process(None, None));
        assert!(!is_same_process(Some(0), Some(0)));
        assert!(!is_same_process(Some(42), None));
        assert!(!is_same_process(Some(42), Some(0)));
        assert!(is_same_process(Some(42), Some(42)));
    }

    #[test]
    fn tracked_exit_is_only_handled_for_the_tracked_process() {
        assert!(should_handle_tracked_server_exit(Some(10), Some(10)));
        assert!(!should_handle_tracked_server_exit(Some(10), Some(11)));
        assert!(!should_handle_tracked_server_exit(None, Some(11)));
    }

    #[test]
    fn superseded_server_is_killed_only_when_actually_replaced() {
        assert!(should_kill_superseded_server(Some(10), Some(11)));
        assert!(!should_kill_superseded_server(Some(10), Some(10)));
        assert!(!should_kill_superseded_server(None, Some(11)));
        assert!(!should_kill_superseded_server(Some(10), None));
    }

    #[test]
    fn previous_server_is_restored_after_a_lost_race() {
        // The attempt is still tracked, so rolling back to the previous process
        // is safe.
        assert!(should_restore_previous_tracked_server(
            Some(10),
            Some(11),
            Some(11)
        ));
        // Something else already took over tracking; do not clobber it.
        assert!(!should_restore_previous_tracked_server(
            Some(10),
            Some(11),
            Some(12)
        ));
        // Nothing to restore.
        assert!(!should_restore_previous_tracked_server(
            None,
            Some(11),
            Some(11)
        ));
        // The attempt *is* the previous process.
        assert!(!should_restore_previous_tracked_server(
            Some(11),
            Some(11),
            Some(11)
        ));
    }

    #[test]
    fn attempted_server_is_stopped_only_while_tracked() {
        assert!(should_stop_attempted_server(Some(11), Some(11)));
        assert!(!should_stop_attempted_server(Some(11), Some(12)));
        assert!(!should_stop_attempted_server(Some(11), None));
    }
}
