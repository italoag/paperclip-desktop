//! Starting, watching, and stopping the embedded Paperclip server.
//!
//! Port of `startServer` / `killServer` and the output plumbing inside
//! `bootLocal` in `src/main.ts`.
//!
//! The server is an opaque Node process we do not own, so this module keeps the
//! contract with it deliberately narrow: spawn it with the right environment,
//! mirror everything it prints into `server.log`, sniff a few well-known lines
//! for progress, and keep the last chunk of output around for the failure
//! dialog.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::server::agent_secret::AgentJwtSecret;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc::UnboundedSender;

use super::boot_progress::{BootProgress, BootStatus, OutputTail};
use super::process::{self, SpawnError};

/// Where the server entry point lives, relative to the app root.
pub fn entry_point(app_root: &Path, packaged: bool) -> PathBuf {
    if packaged {
        app_root.join("server").join("dist").join("index.js")
    } else {
        app_root
            .join("node_modules")
            .join("@paperclipai")
            .join("server")
            .join("dist")
            .join("index.js")
    }
}

/// Resolves symlinks in the entry point before it is executed.
///
/// # Why this is not optional
///
/// The server only runs its startup path when it believes it is the process
/// entry point:
///
/// ```js
/// // @paperclipai/server/dist/index.js
/// function isMainModule(metaUrl) {
///   return pathToFileURL(resolve(process.argv[1])).href === metaUrl;
/// }
/// if (isMainModule(import.meta.url)) { void startServer() }
/// ```
///
/// Node resolves symlinks when computing `import.meta.url` for an ES module, but
/// `process.argv[1]` is whatever path we passed. Under pnpm,
/// `node_modules/@paperclipai/server` is a symlink into `.pnpm/…`, so the two
/// disagree, `isMainModule` is false, and the process **exits 0 having printed
/// nothing** — no error, no server, no clue.
///
/// The packaged bundle is unaffected because `prepare-server.mjs` builds it with
/// `npm install --production`, which writes a flat tree. Passing the canonical
/// path fixes development without changing packaged behaviour.
pub fn resolve_entry_point(entry_point: &Path) -> PathBuf {
    std::fs::canonicalize(entry_point).unwrap_or_else(|_| entry_point.to_path_buf())
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub node_binary: PathBuf,
    pub entry_point: PathBuf,
    pub working_dir: PathBuf,
    pub port: u16,
    pub paperclip_home: PathBuf,
    pub path_env: String,
    pub packaged: bool,
    /// Without this the server mints no agent tokens at all; see
    /// `server::agent_secret`.
    pub agent_jwt_secret: AgentJwtSecret,
}

/// Environment overlaid on the inherited one.
///
/// `PAPERCLIP_MIGRATION_AUTO_APPLY` matters: the desktop app has no other way to
/// run migrations, so a version bump would otherwise boot against a stale schema.
///
/// `PAPERCLIP_AGENT_JWT_SECRET` matters just as much and fails far more quietly:
/// without it the server boots fine but mints no agent tokens, and every agent
/// run gets `401 Agent authentication required` on any control-plane write.
pub fn environment(config: &ServerConfig) -> HashMap<String, String> {
    HashMap::from([
        ("PATH".to_string(), config.path_env.clone()),
        (
            "NODE_ENV".to_string(),
            if config.packaged {
                "production"
            } else {
                "development"
            }
            .to_string(),
        ),
        ("PORT".to_string(), config.port.to_string()),
        (
            "PAPERCLIP_HOME".to_string(),
            config.paperclip_home.display().to_string(),
        ),
        (
            "PAPERCLIP_MIGRATION_AUTO_APPLY".to_string(),
            "true".to_string(),
        ),
        // `better-auth` reads this too, through its own
        // `BETTER_AUTH_SECRET ?? PAPERCLIP_AGENT_JWT_SECRET` fallback, so the
        // specific name covers both without overriding a value an operator set.
        (
            "PAPERCLIP_AGENT_JWT_SECRET".to_string(),
            config.agent_jwt_secret.expose().to_string(),
        ),
    ])
}

pub struct RunningServer {
    child: Child,
    pub port: u16,
    pub pid: Option<u32>,
    tail: Arc<Mutex<OutputTail>>,
}

impl RunningServer {
    /// The most recent server output, for the failure dialog.
    pub fn tail(&self, lines: usize) -> String {
        self.tail
            .lock()
            .map(|tail| tail.tail(lines))
            .unwrap_or_default()
    }

    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)) | Err(_))
    }

    /// Terminates the whole process group, escalating if it does not comply.
    pub async fn shutdown(mut self) -> bool {
        process::shutdown(&mut self.child, process::SHUTDOWN_GRACE).await
    }

    /// Resolves when the process exits on its own.
    pub async fn wait(&mut self) -> Option<i32> {
        self.child
            .wait()
            .await
            .ok()
            .and_then(|status| status.code())
    }
}

/// Spawns the server and starts mirroring its output.
pub fn spawn(
    config: &ServerConfig,
    log_file: Option<std::fs::File>,
    status: UnboundedSender<BootStatus>,
) -> Result<RunningServer, SpawnError> {
    let entry_point = resolve_entry_point(&config.entry_point);
    let mut command = process::command(
        &config.node_binary,
        &[entry_point.as_path()],
        &config.working_dir,
    );
    command.envs(environment(config));

    let mut child = command.spawn()?;
    let pid = child.id();
    let port = config.port;

    let tail = Arc::new(Mutex::new(OutputTail::default()));
    let progress = Arc::new(Mutex::new(BootProgress::new()));
    let log = log_file.map(|file| Arc::new(Mutex::new(file)));

    if let Some(stdout) = child.stdout.take() {
        pump(
            stdout,
            tail.clone(),
            progress.clone(),
            log.clone(),
            status.clone(),
        );
    }
    if let Some(stderr) = child.stderr.take() {
        pump(stderr, tail.clone(), progress, log, status);
    }

    log::info!("started Paperclip server (pid={pid:?}, port={port})");

    Ok(RunningServer {
        child,
        port,
        pid,
        tail,
    })
}

/// Streams one output pipe into the log, the tail buffer, and the progress bar.
fn pump<R>(
    reader: R,
    tail: Arc<Mutex<OutputTail>>,
    progress: Arc<Mutex<BootProgress>>,
    log: Option<Arc<Mutex<std::fs::File>>>,
    status: UnboundedSender<BootStatus>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(log) = log.as_ref() {
                if let Ok(mut file) = log.lock() {
                    let _ = writeln!(file, "{line}");
                }
            }

            if let Ok(mut tail) = tail.lock() {
                tail.push_chunk(&line);
            }

            let update = progress
                .lock()
                .ok()
                .and_then(|mut progress| progress.observe_output(&line));
            if let Some(update) = update {
                // A closed receiver just means the launcher is gone.
                let _ = status.send(update);
            }
        }
    });
}

/// Header written to `server.log` at the start of every run.
pub fn log_banner(port: u16, now: &str) -> String {
    format!("\n--- Server start {now} (port={port}) ---\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ServerConfig {
        ServerConfig {
            node_binary: PathBuf::from("/usr/bin/node"),
            entry_point: PathBuf::from("/app/server/dist/index.js"),
            working_dir: PathBuf::from("/app"),
            port: 3100,
            paperclip_home: PathBuf::from("/Users/example/.paperclip"),
            path_env: "/usr/bin:/bin".to_string(),
            packaged: true,
            agent_jwt_secret: AgentJwtSecret::ephemeral(),
        }
    }

    #[test]
    fn entry_point_differs_between_packaged_and_development_layouts() {
        let root = Path::new("/app");

        assert_eq!(
            entry_point(root, true),
            PathBuf::from("/app/server/dist/index.js")
        );
        assert_eq!(
            entry_point(root, false),
            PathBuf::from("/app/node_modules/@paperclipai/server/dist/index.js")
        );
    }

    #[test]
    fn the_environment_carries_everything_the_server_needs() {
        let env = environment(&config());

        assert_eq!(env["PORT"], "3100");
        assert_eq!(env["NODE_ENV"], "production");
        assert_eq!(env["PATH"], "/usr/bin:/bin");
        assert_eq!(env["PAPERCLIP_HOME"], "/Users/example/.paperclip");
        // Without this the app cannot migrate its own schema.
        assert_eq!(env["PAPERCLIP_MIGRATION_AUTO_APPLY"], "true");
    }

    /// The gap this closes: the server boots and serves the UI without this
    /// variable, but mints no agent tokens, so every control-plane write from an
    /// agent returns `401 Agent authentication required`.
    #[test]
    fn environment_carries_the_agent_jwt_secret() {
        let mut config = config();
        config.agent_jwt_secret = AgentJwtSecret::ephemeral();
        let expected = config.agent_jwt_secret.expose().to_string();

        let env = environment(&config);

        assert_eq!(env.get("PAPERCLIP_AGENT_JWT_SECRET"), Some(&expected));
    }

    #[test]
    fn development_builds_report_a_development_node_env() {
        let env = environment(&ServerConfig {
            packaged: false,
            ..config()
        });
        assert_eq!(env["NODE_ENV"], "development");
    }

    /// Regression guard for the silent-no-op described on [`resolve_entry_point`]:
    /// a symlinked entry point must be handed to Node as its real location.
    #[test]
    fn the_entry_point_is_resolved_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real-index.js");
        std::fs::write(&real, "").unwrap();

        let link = dir.path().join("linked-index.js");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(
            resolve_entry_point(&link),
            std::fs::canonicalize(&real).unwrap()
        );
    }

    #[test]
    fn an_unresolvable_entry_point_is_passed_through_unchanged() {
        let missing = Path::new("/nonexistent/server/dist/index.js");
        assert_eq!(resolve_entry_point(missing), missing);
    }

    #[test]
    fn the_log_banner_records_the_port_and_time() {
        let banner = log_banner(3123, "2026-04-06T10:00:00.000Z");
        assert!(banner.contains("port=3123"));
        assert!(banner.contains("2026-04-06T10:00:00.000Z"));
    }

    /// End-to-end over a stand-in that prints the real startup markers: output
    /// must reach the log file, the tail buffer, and the progress channel.
    #[tokio::test]
    async fn spawning_streams_output_to_the_log_tail_and_progress() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("server.log");
        let log = std::fs::File::create(&log_path).unwrap();

        let script = dir.path().join("fake-server.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             echo \"boot line\"\n\
             echo \"PostgreSQL ready\"\n\
             echo \"Server listening on http://localhost:$PORT\"\n\
             echo \"PAPERCLIP_HOME=$PAPERCLIP_HOME\"\n\
             sleep 30\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let config = ServerConfig {
            node_binary: script.clone(),
            entry_point: PathBuf::from("ignored"),
            working_dir: dir.path().to_path_buf(),
            port: 3199,
            paperclip_home: dir.path().join("home"),
            path_env: "/usr/bin:/bin".to_string(),
            packaged: true,
            agent_jwt_secret: AgentJwtSecret::ephemeral(),
        };

        let server = spawn(&config, Some(log), sender).unwrap();
        assert!(server.pid.is_some());

        let first = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
            .await
            .expect("no progress update")
            .unwrap();
        assert_eq!(first.progress, 35, "PostgreSQL marker should fire first");

        let second = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
            .await
            .expect("no second progress update")
            .unwrap();
        assert_eq!(second.progress, 55, "listening marker should follow");

        // Let the remaining lines drain.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let tail = server.tail(10);
        assert!(tail.contains("boot line"), "tail was: {tail}");
        assert!(
            tail.contains(&format!(
                "PAPERCLIP_HOME={}",
                config.paperclip_home.display()
            )),
            "environment did not reach the child: {tail}"
        );

        assert!(server.shutdown().await);

        let logged = std::fs::read_to_string(&log_path).unwrap();
        assert!(logged.contains("Server listening on"), "log was: {logged}");
    }

    #[tokio::test]
    async fn spawning_a_missing_binary_is_reported_rather_than_panicking() {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let result = spawn(
            &ServerConfig {
                node_binary: PathBuf::from("/nonexistent/node"),
                ..config()
            },
            None,
            sender,
        );

        assert!(matches!(result, Err(SpawnError::Spawn(_))));
    }
}
