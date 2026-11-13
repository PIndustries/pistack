// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The agent side of the protocol: what `anvil` uses to talk to the control
//! plane.

use std::sync::{Arc, Mutex};
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

/// How long to wait for a server to accept a connection. Short, so a
/// powered-off server (whose packets vanish rather than being refused) is
/// skipped quickly in favour of the next known one.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Where the agent routes are mounted under a server's API address.
pub const RPC_PATH: &str = "/rpc";

/// Most "not the leader" answers one call follows before giving up.
const MAX_LEADER_HOPS: usize = 3;

/// Append [`RPC_PATH`] to a server's API address unless it is already there.
pub fn rpc_base(api: &str) -> String {
    let a = api.trim().trim_end_matches('/');
    if a.ends_with(RPC_PATH) {
        a.to_owned()
    } else {
        format!("{a}{RPC_PATH}")
    }
}

/// The server this client talks to now, plus every server it has heard of.
/// Shared by clones so the whole agent moves together.
#[derive(Debug)]
struct Servers {
    current: String,
    known: Vec<String>,
    /// The agent joined over https. It then never moves to a plain-HTTP
    /// address, whoever names it: its client certificate cannot be shown
    /// there, and whatever it sent would cross the network in the clear.
    secure: bool,
    /// Plain-HTTP addresses already refused, so each is logged once.
    refused: Vec<String>,
}

impl Servers {
    /// Remember `base` unless it would be a downgrade. False when refused.
    fn learn(&mut self, base: String) -> bool {
        if !self.allows(&base) {
            return false;
        }
        if !self.known.contains(&base) {
            self.known.push(base);
        }
        true
    }

    fn allows(&mut self, base: &str) -> bool {
        if !self.secure || is_https(base) {
            return true;
        }
        if !self.refused.iter().any(|r| r == base) {
            tracing::warn!(
                server = base,
                "agent rpc: joined over https, refusing a plain-http server address"
            );
            self.refused.push(base.to_owned());
        }
        false
    }
}

fn is_https(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).is_ok_and(|u| u.scheme() == "https")
}

#[derive(Clone)]
pub struct AgentClient {
    http: reqwest::Client,
    servers: Arc<Mutex<Servers>>,
    node_name: String,
    token: Option<String>,
}

impl std::fmt::Debug for AgentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentClient")
            .field("base", &self.base_url())
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
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(format!("pistack-anvil/{}", pistack_types::BUILD))
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
            servers: Arc::new(Mutex::new(Servers {
                secure: is_https(&base),
                current: base.clone(),
                known: vec![base],
                refused: Vec::new(),
            })),
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
        let body = EnrollRequest {
            node_name: node_name.to_owned(),
        };
        self.send(|base| {
            self.http
                .post(format!("{base}/agents/enroll"))
                .json(&body)
                .timeout(Duration::from_secs(30))
        })
        .await
    }

    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// The server this client talks to now. It changes when a server
    /// answers "not the leader" or the current one stops answering.
    pub fn base_url(&self) -> String {
        self.lock().current.clone()
    }

    /// Every server this client has heard of, current one included.
    pub fn known_servers(&self) -> Vec<String> {
        self.lock().known.clone()
    }

    /// Remember more servers (API addresses or `/rpc` bases) to try when the
    /// current one stops answering. A client that joined over https skips
    /// plain-HTTP addresses.
    pub fn learn_servers<I, S>(&self, servers: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut s = self.lock();
        for api in servers {
            let api = api.as_ref();
            if api.trim().is_empty() || reqwest::Url::parse(api.trim()).is_err() {
                continue;
            }
            s.learn(rpc_base(api));
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Servers> {
        self.servers.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(test)]
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url())
    }

    /// Switch to `next` if we are still on `from`. False when `next` is a
    /// plain-HTTP address and this client joined over https: it stays put.
    fn switch(&self, from: &str, next: String) -> bool {
        let mut s = self.lock();
        if !s.learn(next.clone()) {
            return false;
        }
        if s.current == from {
            tracing::info!(from, to = %next, "agent rpc: switching server");
            s.current = next;
        }
        true
    }

    /// Move on to the next known server after `from` stopped answering.
    fn rotate(&self, from: &str) {
        let mut s = self.lock();
        if s.current != from || s.known.len() < 2 {
            return;
        }
        let at = s.known.iter().position(|k| k == from).unwrap_or(0);
        let next = s.known[(at + 1) % s.known.len()].clone();
        tracing::warn!(from, to = %next, "agent rpc: server not answering, trying the next one");
        s.current = next;
    }

    /// Send one call, following "not the leader" answers to the leader.
    /// A refused or timed-out connection, a timed-out call, or a "no leader
    /// yet" answer moves the client to the next known server and returns
    /// the error, so the caller's retry lands somewhere new. A follower cut
    /// off from the majority keeps saying "no leader yet"; another server
    /// may already know the winner. A leader address this client refuses
    /// (see [`Servers::secure`]) is treated the same way.
    async fn send<T: DeserializeOwned>(
        &self,
        build: impl Fn(&str) -> RequestBuilder,
    ) -> Result<T, RpcError> {
        let mut hops = 0;
        loop {
            let base = self.base_url();
            let result = match self.prepare(build(&base)).send().await {
                Ok(resp) => decode(resp).await,
                Err(e) => {
                    if e.is_connect() || e.is_timeout() {
                        self.rotate(&base);
                    }
                    return Err(RpcError::Http(e));
                }
            };
            if let Err(RpcError::Api { status: 503, error }) = &result {
                if leader_hint(error).is_none() {
                    self.rotate(&base);
                    return result;
                }
            }
            if hops < MAX_LEADER_HOPS {
                if let Err(RpcError::Api { error, .. }) = &result {
                    if let Some(next) = leader_hint(error) {
                        if next != base {
                            if !self.switch(&base, next) {
                                self.rotate(&base);
                                return result;
                            }
                            hops += 1;
                            continue;
                        }
                    }
                }
            }
            return result;
        }
    }

    /// Open the bytes of catalog image `id`. The body is not read here: the
    /// caller streams it with [`reqwest::Response::chunk`]. A follower's
    /// leader hint is followed like any other call.
    pub async fn open_image(&self, id: &str) -> Result<Response, RpcError> {
        let mut hops = 0;
        loop {
            let base = self.base_url();
            // No request timeout: an image can take minutes. The caller
            // gives up when the body stalls.
            let request = self.http.get(format!("{base}/images/{id}/data"));
            let resp = match self.prepare(request).send().await {
                Ok(resp) => resp,
                Err(e) => {
                    if e.is_connect() || e.is_timeout() {
                        self.rotate(&base);
                    }
                    return Err(RpcError::Http(e));
                }
            };
            if resp.status().is_success() {
                return Ok(resp);
            }
            let err = api_error(resp).await;
            if hops < MAX_LEADER_HOPS {
                if let RpcError::Api { error, .. } = &err {
                    if let Some(next) = leader_hint(error) {
                        if next != base {
                            if !self.switch(&base, next) {
                                self.rotate(&base);
                                return Err(err);
                            }
                            hops += 1;
                            continue;
                        }
                    }
                }
            }
            return Err(err);
        }
    }

    fn prepare(&self, rb: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(t) => rb.header(JOIN_HEADER, t),
            None => rb,
        }
    }

    pub async fn hello(&self, hello: &AgentHello) -> Result<HelloAck, RpcError> {
        self.send(|base| {
            self.http
                .post(format!("{base}/agents/hello"))
                .json(hello)
                .timeout(Duration::from_secs(30))
        })
        .await
    }

    pub async fn heartbeat(&self, hb: &Heartbeat) -> Result<HeartbeatAck, RpcError> {
        let ack: HeartbeatAck = self
            .send(|base| {
                self.http
                    .post(format!("{base}/agents/heartbeat"))
                    .json(hb)
                    .timeout(Duration::from_secs(30))
            })
            .await?;
        self.learn_servers(&ack.servers);
        Ok(ack)
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
        let query = [("wait", wait_secs.to_string()), ("max", max.to_string())];
        self.send(|base| {
            self.http
                .get(format!("{base}/agents/{}/commands", self.node_name))
                .query(&query)
                .timeout(Duration::from_secs(wait_secs) + POLL_GRACE)
        })
        .await
    }

    /// Report results for commands this node received.
    pub async fn report(&self, results: &[CommandResult]) -> Result<ReportAck, RpcError> {
        self.send(|base| {
            self.http
                .post(format!("{base}/agents/{}/results", self.node_name))
                .json(results)
                .timeout(Duration::from_secs(30))
        })
        .await
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
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(format!("pistack-anvil/{}", pistack_types::BUILD))
        .identity(identity);
    for cert in reqwest::Certificate::from_pem_bundle(ca_pem.as_bytes()).map_err(RpcError::Http)? {
        builder = builder.add_root_certificate(cert);
    }
    Ok(builder.build()?)
}

/// The leader's `/rpc` base from a "not the leader" answer, if it names one.
fn leader_hint(error: &ApiError) -> Option<String> {
    let leader = error.details.get("leader")?.as_str()?.trim();
    if leader.is_empty() || reqwest::Url::parse(leader).is_err() {
        return None;
    }
    Some(rpc_base(leader))
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
    Err(api_error(resp).await)
}

/// The error a non-success answer carries.
async fn api_error(resp: Response) -> RpcError {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let error = match serde_json::from_str::<Envelope>(&body) {
        Ok(env) => env.error,
        Err(_) => synthesize(status, &body),
    };
    RpcError::Api {
        status: status.as_u16(),
        error,
    }
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
    use axum::response::IntoResponse;

    /// A server that answers every call "not the leader" and names `leader`.
    async fn follower(leader: String) -> String {
        let app = axum::Router::new().fallback(move || {
            let leader = leader.clone();
            async move {
                ApiError::conflict("not the leader")
                    .with_detail("leader", leader)
                    .into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// A leader whose `/rpc` answers polls with nothing and heartbeats with
    /// `servers`.
    async fn leader(servers: Vec<String>) -> String {
        let app = axum::Router::new()
            .route(
                "/rpc/agents/{node}/commands",
                axum::routing::get(|| async { axum::Json(Vec::<Command>::new()) }),
            )
            .route(
                "/rpc/agents/heartbeat",
                axum::routing::post(move || {
                    let servers = servers.clone();
                    async move {
                        axum::Json(HeartbeatAck {
                            server_time: chrono::Utc::now(),
                            commands_waiting: 0,
                            known: true,
                            servers,
                        })
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn beat() -> Heartbeat {
        Heartbeat {
            node_name: "n1".into(),
            resources_free: Default::default(),
            machines: vec![],
            at: chrono::Utc::now(),
            ovn: None,
            usage: None,
        }
    }

    #[tokio::test]
    async fn the_client_follows_a_follower_to_the_leader() {
        let lead = leader(vec!["http://10.0.0.12:8443".into()]).await;
        let follow = follower(lead.clone()).await;
        let c = AgentClient::new(rpc_base(&follow), "n1").unwrap();
        let got = c.next_commands(Duration::from_secs(0)).await.unwrap();
        assert!(got.is_empty());
        assert_eq!(c.base_url(), rpc_base(&lead));
        // Clones share where they talk to.
        let clone = c.clone();
        clone.heartbeat(&beat()).await.unwrap();
        assert_eq!(
            c.known_servers(),
            vec![
                rpc_base(&follow),
                rpc_base(&lead),
                "http://10.0.0.12:8443/rpc".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn a_follower_that_names_itself_is_not_followed_forever() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let me = format!("http://{addr}");
        let named = me.clone();
        let app = axum::Router::new().fallback(move || {
            let named = named.clone();
            async move {
                ApiError::conflict("not the leader")
                    .with_detail("leader", named)
                    .into_response()
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let c = AgentClient::new(rpc_base(&me), "n1").unwrap();
        let err = c.next_commands(Duration::from_secs(0)).await.unwrap_err();
        assert!(matches!(err, RpcError::Api { status: 409, .. }), "{err}");
        assert_eq!(c.base_url(), rpc_base(&me));
    }

    #[tokio::test]
    async fn a_dead_server_moves_the_client_to_the_next_known_one() {
        let lead = leader(vec![]).await;
        // Bind and drop to get a port nothing listens on.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let c = AgentClient::new(rpc_base(&dead), "n1").unwrap();
        c.learn_servers([lead.as_str(), "not a url", ""]);
        assert_eq!(c.known_servers().len(), 2);
        let err = c.next_commands(Duration::from_secs(0)).await.unwrap_err();
        assert!(matches!(err, RpcError::Http(_)), "{err}");
        assert_eq!(c.base_url(), rpc_base(&lead));
        assert!(c
            .next_commands(Duration::from_secs(0))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn no_leader_yet_moves_the_client_to_the_next_known_server() {
        let lead = leader(vec![]).await;
        let app = axum::Router::new().fallback(|| async {
            ApiError::unavailable("no leader yet, retry soon").into_response()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cut_off = format!("http://{addr}");
        let c = AgentClient::new(rpc_base(&cut_off), "n1").unwrap();
        c.learn_servers([lead.as_str()]);
        let err = c.next_commands(Duration::from_secs(0)).await.unwrap_err();
        assert!(matches!(err, RpcError::Api { status: 503, .. }), "{err}");
        assert_eq!(c.base_url(), rpc_base(&lead));
        assert!(c
            .next_commands(Duration::from_secs(0))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_server_that_never_answers_moves_the_client_on() {
        let lead = leader(vec![]).await;
        // Accepts connections but never replies, like a wedged host.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        let silent = format!("http://{addr}");
        let c = AgentClient::new(rpc_base(&silent), "n1").unwrap();
        c.learn_servers([lead.as_str()]);
        // A zero-second poll times out after the grace period.
        let err = c.next_commands(Duration::from_secs(0)).await.unwrap_err();
        assert!(matches!(&err, RpcError::Http(e) if e.is_timeout()), "{err}");
        assert_eq!(c.base_url(), rpc_base(&lead));
    }

    #[test]
    fn a_client_that_joined_over_https_learns_no_plain_http_server() {
        let c = AgentClient::new("https://10.0.0.11:8444/rpc", "n1").unwrap();
        c.learn_servers([
            "http://10.0.0.12:8443",
            "https://10.0.0.12:8444",
            "HTTP://10.0.0.13:8443",
            "https://10.0.0.13:8444",
        ]);
        assert_eq!(
            c.known_servers(),
            vec![
                "https://10.0.0.11:8444/rpc",
                "https://10.0.0.12:8444/rpc",
                "https://10.0.0.13:8444/rpc",
            ]
        );
        // Named as the leader, a plain address is still refused.
        let from = c.base_url();
        assert!(!c.switch(&from, "http://10.0.0.12:8443/rpc".into()));
        assert_eq!(c.base_url(), from);
        assert!(c.switch(&from, "https://10.0.0.12:8444/rpc".into()));
        assert_eq!(c.base_url(), "https://10.0.0.12:8444/rpc");

        // A client that joined over plain HTTP may move either way.
        let plain = AgentClient::new("http://10.0.0.11:8443/rpc", "n1").unwrap();
        plain.learn_servers(["http://10.0.0.12:8443", "https://10.0.0.13:8444"]);
        assert_eq!(plain.known_servers().len(), 3);
    }

    #[tokio::test]
    async fn a_plain_http_leader_is_refused_and_the_next_server_is_tried() {
        let follow = follower("http://10.0.0.13:8443".into()).await;
        let c = AgentClient::new(rpc_base(&follow), "n1").unwrap();
        // The stub speaks plain HTTP; mark the client as one that joined
        // over https, which is all the rule looks at.
        c.lock().secure = true;
        c.learn_servers(["https://10.0.0.12:8444"]);
        let err = c.next_commands(Duration::from_secs(0)).await.unwrap_err();
        assert!(matches!(err, RpcError::Api { status: 409, .. }), "{err}");
        assert_eq!(c.base_url(), "https://10.0.0.12:8444/rpc");
        assert!(!c
            .known_servers()
            .contains(&"http://10.0.0.13:8443/rpc".to_owned()));
    }

    #[test]
    fn rpc_base_adds_the_path_once() {
        assert_eq!(rpc_base("http://a:1"), "http://a:1/rpc");
        assert_eq!(rpc_base("http://a:1/ "), "http://a:1/rpc");
        assert_eq!(rpc_base("http://a:1/rpc/"), "http://a:1/rpc");
    }

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
                        ovn: None,
                        usage: None,
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
