//! The narrow HTTP surface the connection probes need.
//!
//! The TypeScript version took an injectable `fetchImpl`; this trait plays the
//! same role, letting the preflight tests assert exact request URLs and response
//! sequences without a network.
//!
//! Requests deliberately do **not** follow redirects: a remote that bounces
//! `/api/health` somewhere else must not be able to make an arbitrary third
//! party look like a healthy Paperclip instance.

use std::future::Future;
use std::time::Duration;

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResponse {
    pub status: u16,
    /// `None` when the response was not JSON, mirroring the content-type guard
    /// in the original `fetchJson`.
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ProbeError {
    pub message: String,
}

impl ProbeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub trait HttpProbe {
    fn get_json(
        &self,
        url: Url,
        timeout: Duration,
    ) -> impl Future<Output = Result<ProbeResponse, ProbeError>> + Send;
}

/// Production implementation.
#[derive(Debug, Clone)]
pub struct ReqwestProbe {
    client: reqwest::Client,
}

impl ReqwestProbe {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
}

impl HttpProbe for ReqwestProbe {
    async fn get_json(&self, url: Url, timeout: Duration) -> Result<ProbeResponse, ProbeError> {
        let response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(timeout)
            .send()
            .await
            .map_err(|err| ProbeError::new(describe_error(&err)))?;

        let status = response.status().as_u16();
        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().contains("application/json"));

        if !is_json {
            return Ok(ProbeResponse { status, body: None });
        }

        let body = response
            .json::<serde_json::Value>()
            .await
            .map_err(|err| ProbeError::new(describe_error(&err)))?;

        Ok(ProbeResponse {
            status,
            body: Some(body),
        })
    }
}

/// Flattens the error source chain so TLS failures stay recognisable.
///
/// `reqwest`'s own `Display` is generic ("error sending request"); the useful
/// detail — certificate errors in particular — lives further down the chain, and
/// the failure classifier matches on it.
fn describe_error(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(current) = source {
        parts.push(current.to_string());
        source = current.source();
    }
    parts.join(": ")
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Queue-backed probe mirroring the `responses.shift()` fakes in the
    /// TypeScript tests.
    #[derive(Debug, Default)]
    pub struct FakeProbe {
        responses: Mutex<Vec<Result<ProbeResponse, ProbeError>>>,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeProbe {
        pub fn new(responses: Vec<Result<ProbeResponse, ProbeError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn json(status: u16, body: serde_json::Value) -> Result<ProbeResponse, ProbeError> {
            Ok(ProbeResponse {
                status,
                body: Some(body),
            })
        }

        pub fn non_json(status: u16) -> Result<ProbeResponse, ProbeError> {
            Ok(ProbeResponse { status, body: None })
        }

        pub fn failure(message: &str) -> Result<ProbeResponse, ProbeError> {
            Err(ProbeError::new(message))
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl HttpProbe for FakeProbe {
        async fn get_json(
            &self,
            url: Url,
            _timeout: Duration,
        ) -> Result<ProbeResponse, ProbeError> {
            self.calls.lock().unwrap().push(url.to_string());
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(ProbeError::new("fake probe ran out of responses"));
            }
            responses.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_description_includes_the_whole_source_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("outer")]
        struct Outer(#[source] Inner);

        #[derive(Debug, thiserror::Error)]
        #[error("self-signed certificate")]
        struct Inner;

        assert_eq!(
            describe_error(&Outer(Inner)),
            "outer: self-signed certificate"
        );
    }
}
