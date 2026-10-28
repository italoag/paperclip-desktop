//! Supervision of the embedded `@paperclipai/server` Node process.
//!
//! The shell does not implement any Paperclip behaviour; it only owns the
//! lifecycle of the server that does. Split out of `src/main.ts`, where this
//! logic was interleaved with window management.

pub mod adapter_plugins;
pub mod agent_secret;
pub mod boot_progress;
pub mod logging;
pub mod node;
pub mod pid_file;
pub mod ports;
pub mod process;
pub mod supervisor;

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::OnceLock;
    use tokio::sync::Mutex;

    /// Serialises tests that send signals.
    ///
    /// Cargo runs tests as threads of one process, so every test child that is
    /// not its own process-group leader shares the harness's group. Two tests
    /// signalling at once can then reach each other's children — and because
    /// pids are recycled quickly by the rapid spawn/exit loops these tests use,
    /// a `killpg` can even land on a group created moments earlier by another
    /// test. Holding this lock keeps signal delivery deterministic.
    ///
    /// It is a `tokio::sync::Mutex` because the async tests hold it across
    /// `.await` points; use [`signal_lock`] from sync tests.
    pub fn signal_mutex() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Sync-test flavour. Safe because plain `#[test]` functions run without a
    /// Tokio runtime on the thread.
    pub fn signal_lock() -> tokio::sync::MutexGuard<'static, ()> {
        signal_mutex().blocking_lock()
    }

    /// Serialises tests that bind TCP ports.
    ///
    /// They work by binding port 0, noting the ephemeral port, then releasing
    /// it. Run concurrently, one test's released port can be handed straight to
    /// another's `bind_any()`, so a port asserted to be closed suddenly has a
    /// listener on it.
    pub fn port_mutex() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}
