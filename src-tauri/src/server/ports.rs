//! Port discovery for the embedded server.
//!
//! Port of the `isPortInUse` / `findFreePort` / `waitForPort` helpers in
//! `src/main.ts`.
//!
//! These probe by *connecting*, not by binding, exactly like the original. That
//! is intentional: the server binds the port itself moments later, so binding
//! here to test availability would create a race with our own child.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

pub const PREFERRED_PORT: u16 = 3100;
pub const SEARCH_RANGE: u16 = 100;
pub const POLL_INTERVAL: Duration = Duration::from_millis(400);
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Connect timeout for a single probe. The loopback answers immediately or not
/// at all, so this only guards against a pathologically stalled stack.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortError {
    #[error("No free port found in range {start}-{end}")]
    NoFreePort { start: u16, end: u16 },

    #[error("Server did not start within {0}ms")]
    Timeout(u128),
}

pub async fn is_port_in_use(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

pub async fn find_free_port(start: u16) -> Result<u16, PortError> {
    let end = start.saturating_add(SEARCH_RANGE - 1);

    for port in start..=end {
        if !is_port_in_use(port).await {
            return Ok(port);
        }
    }

    Err(PortError::NoFreePort { start, end })
}

/// Blocks until something accepts connections on `port`.
pub async fn wait_for_port(port: u16, timeout: Duration) -> Result<(), PortError> {
    let deadline = Instant::now() + timeout;

    loop {
        if is_port_in_use(port).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(PortError::Timeout(timeout.as_millis()));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_support::port_mutex;
    use tokio::net::TcpListener;

    async fn bind_any() -> (TcpListener, u16) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[tokio::test]
    async fn detects_a_listening_port() {
        let _guard = port_mutex().lock().await;
        let (_listener, port) = bind_any().await;
        assert!(is_port_in_use(port).await);
    }

    #[tokio::test]
    async fn reports_a_closed_port_as_free() {
        let _guard = port_mutex().lock().await;
        let (listener, port) = bind_any().await;
        drop(listener);
        assert!(!is_port_in_use(port).await);
    }

    #[tokio::test]
    async fn find_free_port_skips_ports_in_use() {
        let _guard = port_mutex().lock().await;
        let (_listener, occupied) = bind_any().await;
        let found = find_free_port(occupied).await.unwrap();
        assert_ne!(found, occupied);
        assert!(found > occupied);
    }

    #[tokio::test]
    async fn wait_for_port_returns_once_the_listener_appears() {
        let _guard = port_mutex().lock().await;
        let (listener, port) = bind_any().await;
        drop(listener);

        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            TcpListener::bind((Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap()
        });

        wait_for_port(port, Duration::from_secs(5)).await.unwrap();
        drop(handle.await.unwrap());
    }

    /// Port 1 rather than a released ephemeral port: binding below 1024 needs
    /// root, so nothing on the machine can start listening there mid-test. A
    /// freed ephemeral port can be handed to any process — including another
    /// test — inside the timeout window, which made this assertion flaky.
    #[tokio::test]
    async fn wait_for_port_times_out_when_nothing_listens() {
        const UNBINDABLE_PORT: u16 = 1;

        assert!(
            !is_port_in_use(UNBINDABLE_PORT).await,
            "something is listening on port {UNBINDABLE_PORT}; cannot test the timeout"
        );

        let result = wait_for_port(UNBINDABLE_PORT, Duration::from_millis(200)).await;
        assert!(matches!(result, Err(PortError::Timeout(200))));
    }
}
