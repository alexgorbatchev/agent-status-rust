//! Safe, single-attempt, authenticated Agent Status read requests.
use agent_common::{types::AgentState, usage::Report};
use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, HeaderValue},
    redirect::Policy,
};
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum Cause {
    #[error("requesting agent-server: operation cancelled")]
    Cancelled,
    #[error("agent-server request timed out")]
    Timeout,
    #[error(
        "cannot reach agent-server: connection refused; check the server address and whether agent-server is running"
    )]
    ConnectionRefused,
    #[error("agent-server request failed; check connectivity, TLS, and the HTTP response")]
    Transport,
    #[error("agent-server returned HTTP {0}; check AGENT_STATUS_TOKEN matches the server")]
    Unauthorized(u16),
    #[error("agent-server returned HTTP {0}")]
    Status(u16),
    #[error("invalid {0} from agent-server: unexpected JSON response")]
    Json(&'static str),
    #[error("invalid agent usage from agent-server: agent ID does not match")]
    IdentityMismatch,
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "server URL must be an HTTP(S) base URL without credentials, query parameters, or fragments"
    )]
    InvalidUrl,
    #[error("AGENT_STATUS_TOKEN contains invalid header characters")]
    InvalidToken,
    #[error("request timeout must be positive")]
    InvalidTimeout,
    #[error("could not initialize the agent-server HTTP client")]
    Initialize(#[source] reqwest::Error),
    #[error("GET {endpoint}: {cause}")]
    Request {
        endpoint: Url,
        #[source]
        cause: Cause,
    },
}
#[derive(Clone)]
pub struct ServerClient {
    base: Url,
    token: Option<HeaderValue>,
    client: Client,
}
impl ServerClient {
    pub fn new(base: &str, token: &str, timeout: Duration) -> Result<Self, Error> {
        let base = Url::parse(base).map_err(|_| Error::InvalidUrl)?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::InvalidUrl);
        }
        if timeout.is_zero() {
            return Err(Error::InvalidTimeout);
        }
        let token = if token.is_empty() {
            None
        } else {
            if token.contains(['\r', '\n']) {
                return Err(Error::InvalidToken);
            }
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| Error::InvalidToken)?;
            value.set_sensitive(true);
            Some(value)
        };
        let client = Client::builder()
            .timeout(timeout)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(Error::Initialize)?;
        Ok(Self {
            base,
            token,
            client,
        })
    }
    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        // Construction accepts only HTTP(S), which always supports path segments.
        url.path_segments_mut()
            .expect("HTTP URL has path segments")
            .pop_if_empty()
            .extend(segments.iter().copied());
        url
    }
    async fn get<T: DeserializeOwned>(
        &self,
        cancel: &CancellationToken,
        endpoint: Url,
        label: &'static str,
    ) -> Result<T, Error> {
        let run = async {
            let mut request = self
                .client
                .get(endpoint.clone())
                .header(ACCEPT, "application/json");
            if let Some(value) = &self.token {
                request = request.header(AUTHORIZATION, value.clone());
            }
            let response = request.send().await.map_err(transport_cause)?;
            match response.status().as_u16() {
                200 => (),
                code @ (401 | 403) => return Err(Cause::Unauthorized(code)),
                code => return Err(Cause::Status(code)),
            }
            let bytes = response.bytes().await.map_err(transport_cause)?;
            // from_slice requires exactly one JSON value, followed only by whitespace.
            serde_json::from_slice(&bytes).map_err(|_| Cause::Json(label))
        };
        let result = tokio::select! { biased; () = cancel.cancelled() => Err(Cause::Cancelled), result = run => result };
        result.map_err(|cause| Error::Request { endpoint, cause })
    }
    pub async fn agents(&self, cancel: &CancellationToken) -> Result<Vec<AgentState>, Error> {
        let mut endpoint = self.endpoint(&["api", "v1", "agents"]);
        endpoint
            .query_pairs_mut()
            .append_pair("lifecycle", "RUNNING");
        self.get(cancel, endpoint, "agent list").await
    }
    pub async fn usage(&self, cancel: &CancellationToken, agent_id: &str) -> Result<Report, Error> {
        let endpoint = self.endpoint(&["api", "v1", "agents", agent_id, "usage"]);
        let report: Report = self.get(cancel, endpoint.clone(), "agent usage").await?;
        if report.agent_id != agent_id {
            return Err(Error::Request {
                endpoint,
                cause: Cause::IdentityMismatch,
            });
        }
        Ok(report)
    }
}
fn transport_cause(error: reqwest::Error) -> Cause {
    if error.is_timeout() {
        return Cause::Timeout;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            return Cause::ConnectionRefused;
        }
        source = error.source();
    }
    Cause::Transport
}
