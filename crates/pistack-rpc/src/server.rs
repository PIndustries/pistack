// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The control-plane side of the agent protocol as an axum router.
//!
//! Mount it wherever the gateway serves agents; the paths are relative:
//!
//! ```text
//! POST /agents/enroll
//! POST /agents/hello
//! POST /agents/heartbeat
//! GET  /agents/{node}/commands?wait=30&max=16
//! POST /agents/{node}/results
//! ```
//!
//! # Auth
//!
//! Strict by default: every `/rpc` request except enroll must present a
//! client certificate that the TLS listener verified against the cloud CA
//! ([`keep::PeerIdentity`]). Enroll spends a join token once and returns a
//! CA-signed client certificate for steady-state calls.
//!
//! The certificate's common name is the node name it was enrolled under
//! (see [`pistack_types::node`]). A verified peer may hello, heartbeat,
//! poll, and report only for that node; any other name gets 403.
//!
//! Escape hatch for local HTTP and unit tests: set `PISTACK_RPC_INSECURE=1`
//! to skip the client-certificate check. Join-token enforcement on every
//! call stays opt-in via `PISTACK_REQUIRE_JOIN=1` (authorize only; it does
//! not redeem).

use std::time::Duration;

use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use keep::PeerIdentity;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;

use crate::proto::{
    AgentHello, Command, CommandResult, EnrollRequest, EnrollResponse, Heartbeat, HeartbeatAck,
    HelloAck, ReportAck, DEFAULT_POLL_BATCH, JOIN_HEADER, MAX_LONG_POLL_SECS,
};
use crate::queue::CommandQueue;

/// Set to `1` to make [`router_with_store`] require a join token on every
/// call. Any other value, including unset, leaves join optional.
pub const REQUIRE_JOIN_ENV: &str = "PISTACK_REQUIRE_JOIN";

/// Set to `1` to skip the client-certificate requirement on `/rpc`.
/// Intended only for local development and tests over plain HTTP.
pub const RPC_INSECURE_ENV: &str = "PISTACK_RPC_INSECURE";

/// Auth knobs for the agent router.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RpcAuth {
    /// Require `x-pistack-join` matching an unexpired, unspent token.
    pub require_join: bool,
    /// Require a verified client certificate common name on the connection.
    pub require_client_cert: bool,
}

impl RpcAuth {
    /// Production defaults from the process environment.
    pub fn from_env() -> Self {
        Self {
            require_join: join_required(),
            require_client_cert: client_cert_required(),
        }
    }

    /// No join token and no client certificate (unit tests / open lab).
    pub fn open() -> Self {
        Self {
            require_join: false,
            require_client_cert: false,
        }
    }

    /// Join token only (existing test helper behaviour).
    pub fn join_only() -> Self {
        Self {
            require_join: true,
            require_client_cert: false,
        }
    }

    /// Client certificate only.
    pub fn mtls_only() -> Self {
        Self {
            require_join: false,
            require_client_cert: true,
        }
    }
}

/// Build the agent-facing router over `queue`. Does not require join or mTLS.
pub fn router(queue: CommandQueue) -> Router {
    routes(queue)
}

/// Same routes as [`router`], with auth from [`RpcAuth::from_env`].
///
/// When [`RPC_INSECURE_ENV`] is not exactly `1`, every request except enroll
/// must carry a verified client certificate (see [`keep::PeerIdentity`]).
/// When [`REQUIRE_JOIN_ENV`] is exactly `1`, every request must also carry
/// [`JOIN_HEADER`] matching an unexpired stored join token that is not a
/// spent one-time token. The token is not redeemed on those paths.
pub fn router_with_store(queue: CommandQueue, store: Store) -> Router {
    router_with_extra(queue, store, Router::new())
}

/// [`router_with_store`] plus `extra` routes that belong to the agent
/// transport but need state this crate does not have (the image download).
/// They sit behind the same client-certificate and join-token checks.
pub fn router_with_extra(queue: CommandQueue, store: Store, extra: Router) -> Router {
    let auth = RpcAuth::from_env();
    if auth.require_client_cert {
        tracing::info!("/rpc requires a client certificate (set {RPC_INSECURE_ENV}=1 to disable)");
    }
    if auth.require_join {
        tracing::info!("/rpc requires a join token");
    }
    router_with_auth_and(queue, store, auth, extra)
}

/// Routes with an explicit [`RpcAuth`]. Tests use this so they do not touch
/// the process environment.
pub fn router_with_auth(queue: CommandQueue, store: Store, auth: RpcAuth) -> Router {
    router_with_auth_and(queue, store, auth, Router::new())
}

/// [`router_with_auth`] with `extra` routes behind the same checks.
pub fn router_with_auth_and(
    queue: CommandQueue,
    store: Store,
    auth: RpcAuth,
    extra: Router,
) -> Router {
    let enroll = Router::new()
        .route("/agents/enroll", post(enroll))
        .with_state(store.clone());
    let app = enroll
        .merge(routes(queue).layer(axum::Extension(store.clone())))
        .merge(extra);
    if !auth.require_join && !auth.require_client_cert {
        return app;
    }
    app.layer(middleware::from_fn_with_state(
        AuthState { store, auth },
        require_rpc_auth,
    ))
}

/// Routes that always require a join token and never a client certificate.
/// Tests use this so they do not touch the process environment.
#[cfg(test)]
pub(crate) fn router_guarded(queue: CommandQueue, store: Store) -> Router {
    router_with_auth(queue, store, RpcAuth::join_only())
}

fn routes(queue: CommandQueue) -> Router {
    Router::new()
        .route("/agents/hello", post(hello))
        .route("/agents/heartbeat", post(heartbeat))
        .route("/agents/{node}/commands", get(commands))
        .route("/agents/{node}/results", post(results))
        .with_state(queue)
}

fn join_required() -> bool {
    join_required_value(std::env::var(REQUIRE_JOIN_ENV).ok().as_deref())
}

fn join_required_value(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
}

fn client_cert_required() -> bool {
    client_cert_required_value(std::env::var(RPC_INSECURE_ENV).ok().as_deref())
}

fn client_cert_required_value(insecure: Option<&str>) -> bool {
    insecure.is_none_or(|v| v.trim() != "1")
}

#[derive(Clone)]
struct AuthState {
    store: Store,
    auth: RpcAuth,
}

fn is_enroll(req: &Request) -> bool {
    req.uri().path() == "/agents/enroll" || req.uri().path().ends_with("/agents/enroll")
}

async fn require_rpc_auth(State(state): State<AuthState>, req: Request, next: Next) -> Response {
    let enroll = is_enroll(&req);
    if state.auth.require_join || enroll {
        // Enroll always needs a join token, even when REQUIRE_JOIN is off.
        if let Err(err) = check_join(&state.store, req.headers()) {
            return err.into_response();
        }
    }
    if state.auth.require_client_cert && !enroll {
        if let Err(err) = check_client_cert(req.extensions()) {
            return err.into_response();
        }
    }
    next.run(req).await
}

/// Ask keep whether `x-pistack-join` is usable. Does not write the store
/// and does not log the clear text.
fn check_join(store: &Store, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = presented_token(headers)?;
    accept_join_token(store, token)
}

fn presented_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    match headers.get(JOIN_HEADER) {
        None => Err(ApiError::unauthorized("missing join token")),
        Some(value) => match value.to_str() {
            Ok(s) if !s.trim().is_empty() => Ok(s.trim()),
            Ok(_) => Err(ApiError::unauthorized("missing join token")),
            Err(_) => Err(ApiError::unauthorized("malformed join token")),
        },
    }
}

fn accept_join_token(store: &Store, token: &str) -> Result<(), ApiError> {
    // `token` is clear text. Do not log it, or interpolate it into errors.
    keep::authorize_join_token(store, token)
        .map(|_| ())
        .map_err(ApiError::from)
}

fn check_client_cert(ext: &axum::http::Extensions) -> Result<(), ApiError> {
    let peer = ext
        .get::<ConnectInfo<PeerIdentity>>()
        .map(|c| &c.0)
        .or_else(|| ext.get::<PeerIdentity>());
    match peer.and_then(|p| p.common_name.as_deref()) {
        Some(cn) if !cn.is_empty() => Ok(()),
        _ => Err(ApiError::unauthorized("missing client certificate")),
    }
}

fn check_node_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > pistack_types::node::MAX_NODE_NAME {
        return Err(ApiError::bad_request(
            "node_name must be 1..=253 characters",
        ));
    }
    if !pistack_types::node::is_valid_node_name(name) {
        return Err(ApiError::bad_request(
            "node_name may contain only letters, digits, '-', '.' and '_'",
        ));
    }
    Ok(())
}

/// Bind the node name to the client certificate. Enroll issues the
/// certificate with the node name as its common name, so a verified peer
/// may only speak for that node. Without a verified peer (plain HTTP with
/// `PISTACK_RPC_INSECURE=1`) there is nothing to bind and the name stands.
fn check_peer_node(ext: &axum::http::Extensions, node: &str) -> ApiResult<()> {
    let peer = ext
        .get::<ConnectInfo<PeerIdentity>>()
        .map(|c| &c.0)
        .or_else(|| ext.get::<PeerIdentity>());
    match peer.and_then(|p| p.common_name.as_deref()) {
        Some(cn) if !cn.is_empty() && cn != node => Err(ApiError::forbidden(format!(
            "client certificate is for node {cn}, not {node}"
        ))),
        _ => Ok(()),
    }
}

async fn enroll(
    State(store): State<Store>,
    headers: HeaderMap,
    Json(req): Json<EnrollRequest>,
) -> ApiResult<Json<EnrollResponse>> {
    check_node_name(&req.node_name)?;
    let token = presented_token(&headers)?;
    // Spend-once exchange: redeem before issuing so a raced second enroll
    // cannot mint another certificate with the same one-time token.
    let grant = keep::redeem_join_token(&store, token).map_err(ApiError::from)?;
    let client = keep::Client::open(&store).map_err(ApiError::from)?;
    let pki = client
        .issue_client_cert(&req.node_name)
        .map_err(ApiError::from)?;
    Ok(Json(EnrollResponse {
        cert_pem: pki.cert_pem,
        key_pem: pki.key_pem,
        ca_pem: pki.ca_pem,
        grant,
    }))
}

async fn hello(
    State(q): State<CommandQueue>,
    ext: axum::http::Extensions,
    Json(h): Json<AgentHello>,
) -> ApiResult<Json<HelloAck>> {
    check_node_name(&h.node_name)?;
    check_peer_node(&ext, &h.node_name)?;
    Ok(Json(q.hello(&h)))
}

async fn heartbeat(
    State(q): State<CommandQueue>,
    ext: axum::http::Extensions,
    store: Option<axum::Extension<Store>>,
    Json(hb): Json<Heartbeat>,
) -> ApiResult<Json<HeartbeatAck>> {
    check_node_name(&hb.node_name)?;
    check_peer_node(&ext, &hb.node_name)?;
    let mut ack = q.heartbeat(&hb);
    if let Some(axum::Extension(store)) = store {
        let over_tls = ext.get::<PeerIdentity>().is_some_and(|peer| peer.tls);
        ack.servers = cluster_servers(&store, over_tls);
    }
    Ok(Json(ack))
}

/// Only the fields we need from a cluster member record.
#[derive(Deserialize)]
struct MemberAddress {
    api: String,
    #[serde(default)]
    agent_api: Option<String>,
}

/// The address an agent should use for a server whose member record holds
/// `api` and `agent_api`.
///
/// An agent that called over TLS is sent to the server's TLS listener, the
/// only one where its client certificate can be checked. A server with no
/// TLS listener has only `api`. An agent that called over plain HTTP runs
/// without a certificate and may not trust the cloud CA, so it stays on the
/// plain listener.
pub fn agent_address<'a>(api: &'a str, agent_api: Option<&'a str>, over_tls: bool) -> &'a str {
    match agent_api.map(str::trim).filter(|a| !a.is_empty()) {
        Some(agent_api) if over_tls => agent_api,
        _ => api.trim(),
    }
}

/// Where an agent reaches each of the cluster's servers, from the replicated
/// member list.
fn cluster_servers(store: &Store, over_tls: bool) -> Vec<String> {
    store
        .get_meta::<Vec<MemberAddress>>("members")
        .ok()
        .flatten()
        .unwrap_or_default()
        .iter()
        .map(|m| agent_address(&m.api, m.agent_api.as_deref(), over_tls).to_owned())
        .filter(|address| !address.is_empty())
        .collect()
}

#[derive(Debug, Default, Deserialize)]
pub struct PollQuery {
    /// Seconds to wait for a command; capped at [`MAX_LONG_POLL_SECS`].
    #[serde(default)]
    pub wait: Option<u64>,
    /// Most commands to return in one reply.
    #[serde(default)]
    pub max: Option<usize>,
}

async fn commands(
    State(q): State<CommandQueue>,
    Path(node): Path<String>,
    ext: axum::http::Extensions,
    Query(p): Query<PollQuery>,
) -> ApiResult<Json<Vec<Command>>> {
    check_node_name(&node)?;
    check_peer_node(&ext, &node)?;
    let wait = Duration::from_secs(p.wait.unwrap_or(MAX_LONG_POLL_SECS).min(MAX_LONG_POLL_SECS));
    let max = p.max.unwrap_or(DEFAULT_POLL_BATCH).clamp(1, 256);
    Ok(Json(q.next(&node, wait, max).await))
}

async fn results(
    State(q): State<CommandQueue>,
    Path(node): Path<String>,
    ext: axum::http::Extensions,
    Json(results): Json<Vec<CommandResult>>,
) -> ApiResult<Json<ReportAck>> {
    check_node_name(&node)?;
    check_peer_node(&ext, &node)?;
    let mut ack = ReportAck::default();
    for r in results {
        let id = r.id;
        match q.ack(&node, r) {
            Ok(()) => ack.accepted += 1,
            Err(_) => ack.unknown.push(id),
        }
    }
    Ok(Json(ack))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use axum::Extension;
    use chrono::{Duration as ChronoDuration, Utc};
    use keep::{create_join_token, redeem_join_token, JoinToken, PeerIdentity};
    use pistack_store::Store;
    use pistack_types::Resources;

    use super::*;
    use crate::client::AgentClient;
    use crate::proto::{AgentHello, Heartbeat};
    use crate::queue::CommandQueue;

    #[test]
    fn malformed_join_header_is_not_logged_as_text() {
        let mut headers = HeaderMap::new();
        headers.insert(
            JOIN_HEADER,
            axum::http::HeaderValue::from_bytes(&[0xff]).unwrap(),
        );
        let err = presented_token(&headers).unwrap_err();
        assert_eq!(err.message, "malformed join token");
        assert!(!err.message.contains('\u{ff}'));
        assert_eq!(
            presented_token(&HeaderMap::new()).unwrap_err().message,
            "missing join token"
        );
    }

    #[test]
    fn require_join_env_is_only_one() {
        assert!(!join_required_value(None));
        assert!(!join_required_value(Some("")));
        assert!(!join_required_value(Some("0")));
        assert!(!join_required_value(Some("true")));
        assert!(!join_required_value(Some("yes")));
        assert!(!join_required_value(Some("01")));
        assert!(join_required_value(Some("1")));
        assert!(join_required_value(Some(" 1 ")));
    }

    #[test]
    fn client_cert_required_unless_insecure() {
        assert!(client_cert_required_value(None));
        assert!(client_cert_required_value(Some("")));
        assert!(client_cert_required_value(Some("0")));
        assert!(client_cert_required_value(Some("true")));
        assert!(!client_cert_required_value(Some("1")));
        assert!(!client_cert_required_value(Some(" 1 ")));
    }

    fn hello(name: &str) -> AgentHello {
        AgentHello {
            node_name: name.into(),
            capabilities: vec![],
            resources: Resources::default(),
            version: "test".into(),
            labels: Default::default(),
            underlay_ip: None,
        }
    }

    async fn serve(store: Store) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_guarded(CommandQueue::default(), store);
        let h = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), h)
    }

    async fn serve_auth(
        store: Store,
        auth: RpcAuth,
        peer: Option<PeerIdentity>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut app = router_with_auth(CommandQueue::default(), store, auth);
        if let Some(p) = peer {
            // Production inserts ConnectInfo<PeerIdentity> from the TLS
            // listener; tests inject the same peer type as an Extension.
            app = app.layer(Extension(p));
        }
        let h = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), h)
    }

    async fn call(base: &str, method: &str, path: &str, token: Option<&str>) -> (u16, String) {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let url = format!("{base}{path}");
        let rb = match method {
            "GET" => http.get(url),
            "POST" => http
                .post(url)
                .header("content-type", "application/json")
                .body("{}"),
            other => panic!("bad method {other}"),
        };
        let rb = match token {
            Some(t) => rb.header(JOIN_HEADER, t),
            None => rb,
        };
        let resp = rb.send().await.unwrap();
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        (status, body)
    }

    fn message_is(body: &str, message: &str) {
        assert!(
            body.contains("\"unauthorized\"") && body.contains(message),
            "{body}"
        );
    }

    #[tokio::test]
    async fn guarded_router_accepts_hashed_token_without_burning_it() {
        let store = Store::in_memory().unwrap();
        let (once, once_clear) = create_join_token(&store, "once", "admin", false, None).unwrap();
        let (many, many_clear) = create_join_token(&store, "many", "admin", true, None).unwrap();
        let stamp = Utc::now() - ChronoDuration::minutes(5);
        store
            .collection::<JoinToken>()
            .modify(&many.meta.id, |t| {
                t.status.uses = 2;
                t.status.last_used = Some(stamp);
                Ok(())
            })
            .unwrap();
        let (spent, spent_clear) =
            create_join_token(&store, "spent", "admin", false, None).unwrap();
        store
            .collection::<JoinToken>()
            .modify(&spent.meta.id, |t| {
                t.status.uses = 1;
                Ok(())
            })
            .unwrap();
        let (old, old_clear) = create_join_token(&store, "old", "admin", true, None).unwrap();
        store
            .collection::<JoinToken>()
            .modify(&old.meta.id, |t| {
                t.spec.expires_at = Utc::now() - ChronoDuration::seconds(2);
                Ok(())
            })
            .unwrap();
        let (_burned, burned_clear) =
            create_join_token(&store, "burned", "admin", false, None).unwrap();
        redeem_join_token(&store, &burned_clear).unwrap();

        let (base, server) = serve(store.clone()).await;

        for (method, path) in [
            ("POST", "/agents/hello"),
            ("POST", "/agents/heartbeat"),
            ("GET", "/agents/n1/commands?wait=30"),
            ("POST", "/agents/n1/results"),
            ("GET", "/nope"),
        ] {
            let started = Instant::now();
            let (status, body) = call(&base, method, path, None).await;
            assert_eq!(status, 401, "{method} {path} {body}");
            message_is(&body, "missing join token");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{method} {path} waited {:?}",
                started.elapsed()
            );
        }

        let (status, body) = call(&base, "POST", "/agents/hello", Some("")).await;
        assert_eq!(status, 401, "{body}");
        message_is(&body, "missing join token");

        let (status, body) = call(&base, "POST", "/agents/hello", Some("pij_nope")).await;
        assert_eq!(status, 401, "{body}");
        message_is(&body, "unknown join token");
        assert!(!body.contains("pij_nope"));

        let (status, body) = call(&base, "POST", "/agents/hello", Some(&spent_clear)).await;
        assert_eq!(status, 401, "{body}");
        message_is(&body, "join token already used");
        assert!(!body.contains(&spent_clear));

        let (status, body) = call(&base, "POST", "/agents/hello", Some(&old_clear)).await;
        assert_eq!(status, 401, "{body}");
        message_is(&body, "join token expired");
        assert!(!body.contains(&old_clear));

        let (status, body) = call(&base, "GET", "/agents/n1/commands", Some(&burned_clear)).await;
        assert_eq!(status, 401, "{body}");
        message_is(&body, "join token already used");
        assert!(!body.contains(&burned_clear));
        let burned = store
            .collection::<JoinToken>()
            .list(None)
            .unwrap()
            .into_iter()
            .find(|token| token.meta.name == "burned")
            .unwrap();
        assert_eq!(burned.status.uses, 1);

        // A gate bearer is not a join token, even when the clear text is valid.
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{base}/agents/hello"))
            .bearer_auth(&many_clear)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        let body = resp.text().await.unwrap();
        assert!(!body.contains(&many_clear));
        message_is(&body, "missing join token");

        // Padding is ignored. Later calls (hello, poll, heartbeat, report)
        // do not redeem the one-time token.
        let resp = http
            .post(format!("{base}/agents/hello"))
            .header(JOIN_HEADER, format!("  {once_clear}  "))
            .json(&hello("n1"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
        let client = AgentClient::new(&base, "n1")
            .unwrap()
            .with_token(once_clear.clone());
        client.hello(&hello("n1")).await.unwrap();
        client.next_commands(Duration::ZERO).await.unwrap();
        client
            .heartbeat(&Heartbeat {
                node_name: "n1".into(),
                resources_free: Resources::default(),
                machines: vec![],
                at: Utc::now(),
                ovn: None,
            })
            .await
            .unwrap();
        client.report(&[]).await.unwrap();
        let got = store.collection::<JoinToken>().get(&once.meta.id).unwrap();
        assert_eq!(got.status.uses, 0);
        assert!(got.status.last_used.is_none());
        assert_eq!(
            got.spec.hash,
            keep::crypto::sha256_hex(once_clear.as_bytes())
        );

        let client = AgentClient::new(&base, "n1")
            .unwrap()
            .with_token(many_clear.clone());
        client.hello(&hello("n1")).await.unwrap();
        client.next_commands(Duration::ZERO).await.unwrap();
        let got = store.collection::<JoinToken>().get(&many.meta.id).unwrap();
        assert_eq!(got.status.uses, 2);
        assert_eq!(got.status.last_used, Some(stamp));

        // Valid token, unknown path: the check runs, then the route 404s.
        let (status, body) = call(&base, "GET", "/nope", Some(&many_clear)).await;
        assert_eq!(status, 404, "{body}");
        assert!(!body.contains(&many_clear));

        server.abort();
    }

    #[tokio::test]
    async fn open_auth_leaves_the_router_open() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_auth(
            CommandQueue::default(),
            Store::in_memory().unwrap(),
            RpcAuth::open(),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = AgentClient::new(format!("http://{addr}"), "n1").unwrap();
        client.hello(&hello("n1")).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn mtls_router_rejects_missing_cert_and_accepts_peer() {
        let store = Store::in_memory().unwrap();
        let (base, server) = serve_auth(store.clone(), RpcAuth::mtls_only(), None).await;
        let (status, body) = call(&base, "POST", "/agents/hello", None).await;
        assert_eq!(status, 401, "{body}");
        message_is(&body, "missing client certificate");
        server.abort();

        let peer = PeerIdentity {
            addr: None,
            common_name: Some("n1".into()),
            tls: true,
        };
        let (base, server) = serve_auth(store, RpcAuth::mtls_only(), Some(peer)).await;
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{base}/agents/hello"))
            .json(&hello("n1"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
        server.abort();
    }

    #[tokio::test]
    async fn heartbeat_lists_the_cluster_servers() {
        let store = Store::in_memory().unwrap();
        let auth = RpcAuth {
            require_join: false,
            require_client_cert: false,
        };
        let (base, server) = serve_auth(store.clone(), auth, None).await;
        let http = reqwest::Client::new();
        let beat = Heartbeat {
            node_name: "n1".into(),
            resources_free: Resources::default(),
            machines: vec![],
            at: Utc::now(),
            ovn: None,
        };
        let ack: HeartbeatAck = http
            .post(format!("{base}/agents/heartbeat"))
            .json(&beat)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(ack.servers.is_empty(), "a single server lists no peers");

        store
            .put_meta(
                "members",
                &serde_json::json!([
                    {"id": "a", "api": "https://10.0.0.10:8443", "role": "voter"},
                    {"id": "b", "api": "https://10.0.0.11:8443", "role": "voter"},
                    {"id": "c", "api": " ", "role": "learner"},
                ]),
            )
            .unwrap();
        let ack: HeartbeatAck = http
            .post(format!("{base}/agents/heartbeat"))
            .json(&beat)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            ack.servers,
            vec!["https://10.0.0.10:8443", "https://10.0.0.11:8443"]
        );
        server.abort();
    }

    #[test]
    fn an_agent_is_sent_to_the_listener_it_can_use() {
        let tls = Some("https://10.0.0.10:8444");
        let api = "http://10.0.0.10:8443";
        assert_eq!(agent_address(api, tls, true), "https://10.0.0.10:8444");
        assert_eq!(agent_address(api, tls, false), api);
        // No TLS listener on record: the API address is all there is.
        assert_eq!(agent_address(api, None, true), api);
        assert_eq!(agent_address(api, Some("  "), true), api);
    }

    #[tokio::test]
    async fn heartbeat_over_tls_lists_the_tls_addresses() {
        let store = Store::in_memory().unwrap();
        store
            .put_meta(
                "members",
                &serde_json::json!([
                    {"id": "a", "api": "http://10.0.0.10:8443", "role": "Voter",
                     "agent_api": "https://10.0.0.10:8444"},
                    {"id": "b", "api": "http://10.0.0.11:8443", "role": "Voter",
                     "agent_api": "https://10.0.0.11:8444"},
                    // Written by a server older than the field.
                    {"id": "c", "api": "http://10.0.0.12:8443", "role": "Learner"},
                ]),
            )
            .unwrap();
        let beat = Heartbeat {
            node_name: "n1".into(),
            resources_free: Resources::default(),
            machines: vec![],
            at: Utc::now(),
            ovn: None,
        };
        let http = reqwest::Client::new();

        let peer = PeerIdentity {
            addr: None,
            common_name: Some("n1".into()),
            tls: true,
        };
        let (base, server) = serve_auth(store.clone(), RpcAuth::mtls_only(), Some(peer)).await;
        let ack: HeartbeatAck = http
            .post(format!("{base}/agents/heartbeat"))
            .json(&beat)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            ack.servers,
            vec![
                "https://10.0.0.10:8444",
                "https://10.0.0.11:8444",
                "http://10.0.0.12:8443"
            ]
        );
        server.abort();

        // An agent on the plain listener is not moved to one it may not trust.
        let (base, server) = serve_auth(store, RpcAuth::open(), None).await;
        let ack: HeartbeatAck = http
            .post(format!("{base}/agents/heartbeat"))
            .json(&beat)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            ack.servers,
            vec![
                "http://10.0.0.10:8443",
                "http://10.0.0.11:8443",
                "http://10.0.0.12:8443"
            ]
        );
        server.abort();
    }

    #[tokio::test]
    async fn client_cert_speaks_only_for_its_own_node() {
        let store = Store::in_memory().unwrap();
        let peer = PeerIdentity {
            addr: None,
            common_name: Some("n1".into()),
            tls: true,
        };
        let (base, server) = serve_auth(store, RpcAuth::mtls_only(), Some(peer)).await;
        let http = reqwest::Client::new();

        let resp = http
            .post(format!("{base}/agents/hello"))
            .json(&hello("n2"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403);
        let body = resp.text().await.unwrap();
        assert!(
            body.contains("forbidden") && body.contains("for node n1, not n2"),
            "{body}"
        );

        let resp = http
            .post(format!("{base}/agents/heartbeat"))
            .json(&Heartbeat {
                node_name: "n2".into(),
                resources_free: Resources::default(),
                machines: vec![],
                at: Utc::now(),
                ovn: None,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403);

        let resp = http
            .get(format!("{base}/agents/n2/commands?wait=0"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403);

        let resp = http
            .post(format!("{base}/agents/n2/results"))
            .json(&Vec::<CommandResult>::new())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403);

        // The same certificate works for its own node on every path.
        let client = AgentClient::new(&base, "n1").unwrap();
        client.hello(&hello("n1")).await.unwrap();
        client.next_commands(Duration::ZERO).await.unwrap();
        client
            .heartbeat(&Heartbeat {
                node_name: "n1".into(),
                resources_free: Resources::default(),
                machines: vec![],
                at: Utc::now(),
                ovn: None,
            })
            .await
            .unwrap();
        client.report(&[]).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn no_peer_means_no_binding() {
        // Plain HTTP with PISTACK_RPC_INSECURE=1: no certificate, so the
        // name in the request stands.
        let (base, server) = serve_auth(Store::in_memory().unwrap(), RpcAuth::open(), None).await;
        let client = AgentClient::new(&base, "n7").unwrap();
        client.hello(&hello("n7")).await.unwrap();
        client.next_commands(Duration::ZERO).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn enroll_spends_join_token_and_returns_client_cert() {
        let store = Store::in_memory().unwrap();
        keep::Client::open(&store).unwrap();
        let (_tok, clear) = create_join_token(&store, "rack", "admin", false, None).unwrap();

        let (base, server) = serve_auth(store.clone(), RpcAuth::mtls_only(), None).await;
        let http = reqwest::Client::new();

        let resp = http
            .post(format!("{base}/agents/enroll"))
            .json(&EnrollRequest {
                node_name: "n1".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        message_is(&resp.text().await.unwrap(), "missing join token");

        let resp = http
            .post(format!("{base}/agents/enroll"))
            .header(JOIN_HEADER, "pij_nope")
            .json(&EnrollRequest {
                node_name: "n1".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        let body = resp.text().await.unwrap();
        message_is(&body, "unknown join token");
        assert!(!body.contains("pij_nope"));

        let resp = http
            .post(format!("{base}/agents/enroll"))
            .header(JOIN_HEADER, &clear)
            .json(&EnrollRequest {
                node_name: "n1".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let enrolled: EnrollResponse = resp.json().await.unwrap();
        assert!(enrolled.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(enrolled.key_pem.contains("BEGIN"));
        assert!(enrolled.ca_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(enrolled.grant.token_name, "rack");

        let der = keep::ca::pem_to_der(&enrolled.cert_pem).unwrap();
        let keep_client = keep::Client::open(&store).unwrap();
        assert_eq!(keep_client.verify_client_cert(&der).unwrap(), "n1");

        let resp = http
            .post(format!("{base}/agents/enroll"))
            .header(JOIN_HEADER, &clear)
            .json(&EnrollRequest {
                node_name: "n1".into(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        let body = resp.text().await.unwrap();
        message_is(&body, "join token already used");
        assert!(!body.contains(&clear));

        let (status, body) = call(&base, "POST", "/agents/hello", None).await;
        assert_eq!(status, 401, "{body}");
        message_is(&body, "missing client certificate");

        server.abort();
    }
}
