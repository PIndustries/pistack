// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The agent side of the protocol: what `anvil` uses to talk to the control
//! plane.

use std::time::Duration;

use pistack_types::ApiError;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::error::RpcError;
use crate::proto::{
    AgentHello, Command, CommandResult, Heartbeat, HeartbeatAck, HelloAck, ReportAck,
    DEFAULT_POLL_BATCH, MAX_LONG_POLL_SECS,
};

/// Slack added to a long-poll's request timeout so a server that honours
/// the full wait is not cut off by the client.
const POLL_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct AgentClient {
    http: reqwest::Client,
    base: String,
    node_name: String,
    token: Option<String>,
}

impl std::fmt::Debug for AgentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentClient")
            .field("base", &self.base)
            .field("node_name", &self.node_name)
            .field("token", &self.token.as_ref().map(|_| "<set>"))
            .finish()
    }
}

impl AgentClient {
    /// `base_url` is where the router from [`crate::server`] is mounted,
    /// e.g. `https://cloud.example:8443`. A trailing slash is fine.
    pub fn new(
        base_url: impl Into<String>,
        node_name: impl Into<String>,
    ) -> Result<Self, RpcError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("pistack-anvil/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Self::with_http(http, base_url, node_name)
    }

    /// Like [`AgentClient::new`] with a caller-built `reqwest::Client`
    /// (custom TLS roots, client certificate, proxies). Do not set a
    /// global request timeout shorter than the long-poll wait.
    pub fn with_http(
        http: reqwest::Client,
        base_url: impl Into<String>,
        node_name: impl Into<String>,
    ) -> Result<Self, RpcError> {
        let base = base_url.into();
        let base = base.trim_end_matches('/').to_owned();
        reqwest::Url::parse(&base).map_err(|e| RpcError::InvalidUrl(format!("{base}: {e}")))?;
        Ok(Self {
            http,
            base,
            node_name: node_name.into(),
            token: None,
        })
    }

    /// Send `Authorization: Bearer <token>` on every request.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn prepare(&self, rb: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    pub async fn hello(&self, hello: &AgentHello) -> Result<HelloAck, RpcError> {
        let rb = self
            .http
            .post(self.url("/agents/hello"))
            .json(hello)
            .timeout(Duration::from_secs(30));
        decode(self.prepare(rb).send().await?).await
    }

    pub async fn heartbeat(&self, hb: &Heartbeat) -> Result<HeartbeatAck, RpcError> {
        let rb = self
            .http
            .post(self.url("/agents/heartbeat"))
            .json(hb)
            .timeout(Duration::from_secs(30));
        decode(self.prepare(rb).send().await?).await
    }

    /// Long-poll for commands, waiting up to `wait` (capped at
    /// [`MAX_LONG_POLL_SECS`] by the server). An empty vector is a normal
    /// timeout; call again.
    pub async fn next_commands(&self, wait: Duration) -> Result<Vec<Command>, RpcError> {
        self.next_commands_batch(wait, DEFAULT_POLL_BATCH).await
    }

    pub async fn next_commands_batch(
        &self,
        wait: Duration,
        max: usize,
    ) -> Result<Vec<Command>, RpcError> {
        let wait_secs = wait.as_secs().min(MAX_LONG_POLL_SECS);
        let rb = self
            .http
            .get(self.url(&format!("/agents/{}/commands", self.node_name)))
            .query(&[("wait", wait_secs.to_string()), ("max", max.to_string())])
            .timeout(Duration::from_secs(wait_secs) + POLL_GRACE);
        decode(self.prepare(rb).send().await?).await
    }

    /// Report results for commands this node received.
    pub async fn report(&self, results: &[CommandResult]) -> Result<ReportAck, RpcError> {
        let rb = self
            .http
            .post(self.url(&format!("/agents/{}/results", self.node_name)))
            .json(results)
            .timeout(Duration::from_secs(30));
        decode(self.prepare(rb).send().await?).await
    }

    /// Report one result.
    pub async fn report_one(&self, result: CommandResult) -> Result<ReportAck, RpcError> {
        self.report(std::slice::from_ref(&result)).await
    }
}

#[derive(Deserialize)]
struct Envelope {
    error: ApiError,
}

async fn decode<T: DeserializeOwned>(resp: Response) -> Result<T, RpcError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp.json::<T>().await?);
    }
    let body = resp.text().await.unwrap_or_default();
    let error = match serde_json::from_str::<Envelope>(&body) {
        Ok(env) => env.error,
        Err(_) => synthesize(status, &body),
    };
    Err(RpcError::Api {
        status: status.as_u16(),
        error,
    })
}

fn synthesize(status: StatusCode, body: &str) -> ApiError {
    let msg = if body.is_empty() {
        status.to_string()
    } else {
        format!("{status}: {body}")
    };
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => ApiError::bad_request(msg),
        StatusCode::UNAUTHORIZED => ApiError::unauthorized(msg),
        StatusCode::FORBIDDEN => ApiError::forbidden(msg),
        StatusCode::NOT_FOUND => ApiError::not_found(msg),
        StatusCode::CONFLICT => ApiError::conflict(msg),
        StatusCode::SERVICE_UNAVAILABLE => ApiError::unavailable(msg),
        _ => ApiError::internal(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalisation_and_validation() {
        let c = AgentClient::new("http://127.0.0.1:8443/", "n1").unwrap();
        assert_eq!(c.base_url(), "http://127.0.0.1:8443");
        assert_eq!(c.url("/agents/hello"), "http://127.0.0.1:8443/agents/hello");
        let e = AgentClient::new("not a url", "n1").unwrap_err();
        assert!(matches!(e, RpcError::InvalidUrl(_)), "{e}");
        let c = c.with_token("secret");
        assert!(!format!("{c:?}").contains("secret"));
    }
}
