//! Liveness probe for the embedded local server.
//!
//! Port of `src/connection/local-server-health.ts`. Polled every 30s while a
//! local connection is active so a server that dies or wedges surfaces as a
//! recovery prompt instead of a blank window.

use std::time::Duration;

use url::Url;

use super::http::{HttpProbe, ProbeError};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalServerHealth {
    pub ok: bool,
    pub detail: Option<String>,
}

impl LocalServerHealth {
    fn healthy() -> Self {
        Self {
            ok: true,
            detail: None,
        }
    }

    fn unhealthy(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: Some(detail.into()),
        }
    }
}

pub async fn probe_local_server_health<P: HttpProbe>(
    probe: &P,
    origin: &str,
    timeout: Duration,
) -> LocalServerHealth {
    let url = match Url::parse(origin).and_then(|base| base.join("/api/health")) {
        Ok(url) => url,
        Err(err) => return LocalServerHealth::unhealthy(err.to_string()),
    };

    match probe.get_json(url, timeout).await {
        Err(ProbeError { message }) => LocalServerHealth::unhealthy(message),
        Ok(response) if response.status != 200 => LocalServerHealth::unhealthy(format!(
            "Health endpoint returned HTTP {}.",
            response.status
        )),
        Ok(response) => match response.body {
            None => LocalServerHealth::unhealthy("Health endpoint returned a non-JSON response."),
            Some(body) => match body.get("status").and_then(serde_json::Value::as_str) {
                Some("ok") => LocalServerHealth::healthy(),
                other => LocalServerHealth::unhealthy(format!(
                    "Health endpoint reported status \"{}\".",
                    other.unwrap_or("unknown")
                )),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::http::fake::FakeProbe;
    use serde_json::json;

    const ORIGIN: &str = "http://127.0.0.1:3100";

    #[tokio::test]
    async fn accepts_an_ok_health_payload() {
        let probe = FakeProbe::new(vec![FakeProbe::json(200, json!({ "status": "ok" }))]);
        let result = probe_local_server_health(&probe, ORIGIN, DEFAULT_TIMEOUT).await;

        assert_eq!(result, LocalServerHealth::healthy());
        assert_eq!(probe.calls(), vec!["http://127.0.0.1:3100/api/health"]);
    }

    #[tokio::test]
    async fn rejects_unhealthy_payloads() {
        let probe = FakeProbe::new(vec![FakeProbe::json(200, json!({ "status": "starting" }))]);
        let result = probe_local_server_health(&probe, ORIGIN, DEFAULT_TIMEOUT).await;

        assert!(!result.ok);
        assert!(result.detail.unwrap().contains("starting"));
    }

    #[tokio::test]
    async fn reports_transport_failures() {
        let probe = FakeProbe::new(vec![FakeProbe::failure("socket hang up")]);
        let result = probe_local_server_health(&probe, ORIGIN, DEFAULT_TIMEOUT).await;

        assert!(!result.ok);
        assert!(result.detail.unwrap().contains("socket hang up"));
    }

    #[tokio::test]
    async fn rejects_non_200_and_non_json_responses() {
        let probe = FakeProbe::new(vec![FakeProbe::json(503, json!({ "status": "ok" }))]);
        let result = probe_local_server_health(&probe, ORIGIN, DEFAULT_TIMEOUT).await;
        assert!(result.detail.unwrap().contains("HTTP 503"));

        let probe = FakeProbe::new(vec![FakeProbe::non_json(200)]);
        let result = probe_local_server_health(&probe, ORIGIN, DEFAULT_TIMEOUT).await;
        assert!(result.detail.unwrap().contains("non-JSON"));
    }
}
