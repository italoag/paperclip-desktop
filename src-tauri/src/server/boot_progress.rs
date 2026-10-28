//! Turns the embedded server's stdout into launcher progress updates.
//!
//! Port of the progress bookkeeping inside `bootLocal` in `src/main.ts`. The
//! server has no structured startup protocol, so the original sniffed log lines;
//! that heuristic is reproduced verbatim rather than improved, because changing
//! it would silently change the progress bar against a server we do not own.
//!
//! Progress is strictly monotonic: a late-arriving update can never make the bar
//! jump backwards.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BootStep {
    Init,
    Database,
    Server,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BootStatus {
    pub step: BootStep,
    pub detail: String,
    pub progress: u8,
}

/// Log fragments that mark a startup milestone.
const DATABASE_MARKERS: [&str; 2] = ["PostgreSQL ready", "migration"];
const LISTENING_MARKER: &str = "Server listening on";

#[derive(Debug, Default)]
pub struct BootProgress {
    last_progress: u8,
    database_ready: bool,
    server_listening: bool,
}

impl BootProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Emits a status only when it moves the bar forward.
    pub fn advance(&mut self, step: BootStep, detail: &str, progress: u8) -> Option<BootStatus> {
        if progress <= self.last_progress {
            return None;
        }
        self.last_progress = progress;
        Some(BootStatus {
            step,
            detail: detail.to_string(),
            progress,
        })
    }

    /// Interprets a chunk of server output.
    pub fn observe_output(&mut self, chunk: &str) -> Option<BootStatus> {
        if !self.database_ready && DATABASE_MARKERS.iter().any(|marker| chunk.contains(marker)) {
            self.database_ready = true;
            return self.advance(BootStep::Database, "Running migrations...", 35);
        }

        if !self.server_listening && chunk.contains(LISTENING_MARKER) {
            self.server_listening = true;
            return self.advance(BootStep::Server, "Server is starting...", 55);
        }

        None
    }

    /// The periodic nudge so the bar keeps moving while nothing is logged.
    pub fn tick(&mut self) -> Option<BootStatus> {
        if !self.database_ready {
            self.advance(BootStep::Database, "Launching embedded PostgreSQL...", 20)
        } else if !self.server_listening {
            self.advance(BootStep::Server, "Waiting for server...", 50)
        } else {
            None
        }
    }
}

/// Bounded ring of the most recent server output lines.
///
/// The tail is what gets shown in the "server stopped unexpectedly" dialog, so
/// it has to survive a server that logs continuously for hours without growing
/// without bound.
#[derive(Debug)]
pub struct OutputTail {
    lines: std::collections::VecDeque<String>,
    capacity: usize,
}

impl OutputTail {
    pub const DEFAULT_CAPACITY: usize = 200;
    pub const DIALOG_LINES: usize = 30;

    pub fn new(capacity: usize) -> Self {
        Self {
            lines: std::collections::VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push_chunk(&mut self, chunk: &str) {
        for line in chunk.split('\n').filter(|line| !line.is_empty()) {
            if self.lines.len() == self.capacity {
                self.lines.pop_front();
            }
            self.lines.push_back(line.to_string());
        }
    }

    pub fn tail(&self, count: usize) -> String {
        let skip = self.lines.len().saturating_sub(count);
        self.lines
            .iter()
            .skip(skip)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for OutputTail {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_never_moves_backwards() {
        let mut progress = BootProgress::new();

        assert_eq!(
            progress.advance(BootStep::Init, "Preparing environment...", 5),
            Some(BootStatus {
                step: BootStep::Init,
                detail: "Preparing environment...".into(),
                progress: 5,
            })
        );
        assert!(progress.advance(BootStep::Server, "later", 50).is_some());
        assert_eq!(progress.advance(BootStep::Database, "earlier", 20), None);
        assert_eq!(progress.advance(BootStep::Server, "same", 50), None);
        assert!(progress.advance(BootStep::Ready, "Ready!", 100).is_some());
    }

    #[test]
    fn database_markers_are_recognised_once() {
        for marker in ["PostgreSQL ready on port 5432", "applying migration 003"] {
            let mut progress = BootProgress::new();
            let status = progress.observe_output(marker).unwrap();
            assert_eq!(status.step, BootStep::Database);
            assert_eq!(status.progress, 35);
            // A second sighting must not re-fire.
            assert_eq!(progress.observe_output(marker), None);
        }
    }

    #[test]
    fn the_listening_marker_advances_to_the_server_step() {
        let mut progress = BootProgress::new();
        progress.observe_output("PostgreSQL ready");

        let status = progress
            .observe_output("Server listening on http://localhost:3100")
            .unwrap();
        assert_eq!(status.step, BootStep::Server);
        assert_eq!(status.progress, 55);
    }

    #[test]
    fn unrelated_output_produces_nothing() {
        let mut progress = BootProgress::new();
        assert_eq!(progress.observe_output("some unrelated log line"), None);
    }

    #[test]
    fn ticks_track_whichever_milestone_is_outstanding() {
        let mut progress = BootProgress::new();

        assert_eq!(progress.tick().unwrap().progress, 20);
        // Already at 20; the same tick cannot fire twice.
        assert_eq!(progress.tick(), None);

        progress.observe_output("PostgreSQL ready");
        assert_eq!(progress.tick().unwrap().progress, 50);

        progress.observe_output("Server listening on 3100");
        assert_eq!(progress.tick(), None);
    }

    /// The listening marker can arrive before any database line; progress must
    /// still stay monotonic rather than regressing to the database step.
    #[test]
    fn a_late_database_marker_cannot_rewind_the_bar() {
        let mut progress = BootProgress::new();

        assert_eq!(
            progress
                .observe_output("Server listening on 3100")
                .unwrap()
                .progress,
            55
        );
        assert_eq!(progress.observe_output("running migration"), None);
    }

    #[test]
    fn the_output_tail_is_bounded_and_keeps_the_newest_lines() {
        let mut tail = OutputTail::new(3);
        tail.push_chunk("one\ntwo\nthree\n");
        tail.push_chunk("four\n");

        assert_eq!(tail.tail(10), "two\nthree\nfour");
        assert_eq!(tail.tail(2), "three\nfour");
    }

    #[test]
    fn the_output_tail_ignores_blank_lines_and_partial_chunks() {
        let mut tail = OutputTail::default();
        tail.push_chunk("alpha\n\n");
        tail.push_chunk("beta");

        assert_eq!(tail.tail(10), "alpha\nbeta");
    }
}
