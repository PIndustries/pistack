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
    AgentHello, Command, CommandResult, EnrollRequest, EnrollResponse, Heartbeat, HeartbeatAck,
    HelloAck, ReportAck, DEFAULT_POLL_BATCH, JOIN_HEADER, MAX_LONG_POLL_SECS,
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

    /// Send [`JOIN_HEADER`] on every request when `token` is non-empty.
    ///
    /// Blank input clears the token so the header is absent. The clear text
    /// is not written to logs (`Debug` redacts it). This is not an
    /// `Authorization` bearer.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        let trimmed = token.trim();
        self.token = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        };
        self
    }

    /// Build a client that presents `client` over TLS and trusts `ca_pem`.
    ///
    /// Used after [`Self::enroll`] so hello/heartbeat/command calls speak mTLS.
    pub fn with_mtls(
        base_url: impl Into<String>,
        node_name: impl Into<String>,
        ca_pem: &str,
        client: &keep::Pki,
    ) -> Result<Self, RpcError> {
        let http = mtls_http_client(ca_pem, client)?;
        Self::with_http(http, base_url, node_name)
    }

    /// Spend the join token once and receive a CA-signed client certificate.
    pub async fn enroll(&self, node_name: &str) -> Result<EnrollResponse, RpcError> {
        let rb = self
            .http
            .post(self.url("/agents/enroll"))
            .json(&EnrollRequest {
                node_name: node_name.to_owned(),
            })
            .timeout(Duration::from_secs(30));
        decode(self.prepare(rb).send().await?).await
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
            Some(t) => rb.header(JOIN_HEADER, t),
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

fn mtls_http_client(ca_pem: &str, client: &keep::Pki) -> Result<reqwest::Client, RpcError> {
    let mut identity = String::new();
    identity.push_str(&client.cert_pem);
    if !identity.ends_with('\n') {
        identity.push('\n');
    }
    identity.push_str(&client.key_pem);
    let identity = reqwest::Identity::from_pem(identity.as_bytes()).map_err(RpcError::Http)?;
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("pistack-anvil/", env!("CARGO_PKG_VERSION")))
        .identity(identity);
    for cert in reqwest::Certificate::from_pem_bundle(ca_pem.as_bytes()).map_err(RpcError::Http)? {
        builder = builder.add_root_certificate(cert);
    }
    Ok(builder.build()?)
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
        assert!(format!("{c:?}").contains("<set>"));
        let blank = c.with_token("  ");
        assert!(!format!("{blank:?}").contains("<set>"));
        assert_eq!(JOIN_HEADER, "x-pistack-join");
    }

    #[tokio::test]
    async fn join_header_on_every_rpc_and_absent_when_empty() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let record = record.clone();
            async move {
                let join = req
                    .headers()
                    .get(JOIN_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let auth = req
                    .headers()
                    .contains_key(axum::http::header::AUTHORIZATION);
                record.lock().expect("lock").push((
                    req.method().to_string(),
                    req.uri().path().to_owned(),
                    join,
                    auth,
                ));
                axum::http::StatusCode::NO_CONTENT
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{addr}");

        let send = |token: Option<&str>| {
            let base = base.clone();
            let token = token.map(str::to_owned);
            async move {
                let mut client = AgentClient::new(&base, "n1").unwrap();
                if let Some(t) = token {
                    client = client.with_token(t);
                }
                let hello = AgentHello {
                    node_name: "n1".into(),
                    capabilities: vec![],
                    resources: pistack_types::Resources::default(),
                    version: String::new(),
                    labels: Default::default(),
                    underlay_ip: None,
                };
                let _ = client.hello(&hello).await;
                let _ = client
                    .heartbeat(&Heartbeat {
                        node_name: "n1".into(),
                        resources_free: pistack_types::Resources::default(),
                        machines: vec![],
                        at: chrono::Utc::now(),
                    })
                    .await;
                let _ = client.next_commands(Duration::ZERO).await;
                let _ = client.report(&[CommandResult::ok(uuid::Uuid::nil())]).await;
            }
        };
        send(Some("  pij_secret  ")).await;
        send(Some("")).await;
        send(None).await;

        let got = seen.lock().expect("lock").clone();
        assert_eq!(got.len(), 12, "{got:?}");
        for (i, row) in got.iter().enumerate() {
            assert!(!row.3, "authorization on {row:?}");
            if i < 4 {
                assert_eq!(row.2.as_deref(), Some("pij_secret"), "{row:?}");
            } else {
                assert_eq!(row.2, None, "{row:?}");
            }
        }
        let paths: Vec<&str> = got.iter().take(4).map(|r| r.1.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "/agents/hello",
                "/agents/heartbeat",
                "/agents/n1/commands",
                "/agents/n1/results",
            ]
        );
        server.abort();
    }
}
