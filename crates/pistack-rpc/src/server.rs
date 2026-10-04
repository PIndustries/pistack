// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The control-plane side of the agent protocol as an axum router.
//!
//! Mount it wherever the gateway serves agents; the paths are relative:
//!
//! ```text
//! POST /agents/hello
//! POST /agents/heartbeat
//! GET  /agents/{node}/commands?wait=30&max=16
//! POST /agents/{node}/results
//! ```
//!
//! Join-token enforcement is off unless the router was built with a store
//! and `PISTACK_REQUIRE_JOIN=1`. Unset, `/rpc` is the open router. The check
//! asks keep whether `x-pistack-join` matches an unexpired token that is not
//! spent. It does not redeem, so a long-poll does not burn a one-time token.

use std::time::Duration;

use axum::extract::{Path, Query, Request, State};
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;

use crate::proto::{
    AgentHello, Command, CommandResult, Heartbeat, HeartbeatAck, HelloAck, ReportAck,
    DEFAULT_POLL_BATCH, JOIN_HEADER, MAX_LONG_POLL_SECS,
};
use crate::queue::CommandQueue;

/// Set to `1` to make [`router_with_store`] require a join token.
/// Any other value, including unset, leaves the router open.
pub const REQUIRE_JOIN_ENV: &str = "PISTACK_REQUIRE_JOIN";

/// Build the agent-facing router over `queue`. Does not require a join token.
pub fn router(queue: CommandQueue) -> Router {
    routes(queue)
}

/// Same routes as [`router`]. When [`REQUIRE_JOIN_ENV`] is exactly `1`, every
/// request must carry [`JOIN_HEADER`] matching an unexpired stored join token
/// that is not a spent one-time token. The token is not redeemed. Any other
/// env value, including unset, returns the open router and ignores `store`.
pub fn router_with_store(queue: CommandQueue, store: Store) -> Router {
    if join_required() {
        tracing::info!("/rpc requires a join token");
        router_guarded(queue, store)
    } else {
        routes(queue)
    }
}

/// Routes that always require a join token. Tests use this so they do not
/// touch the process environment. The live server goes through
/// [`router_with_store`].
pub(crate) fn router_guarded(queue: CommandQueue, store: Store) -> Router {
    routes(queue).layer(middleware::from_fn_with_state(store, require_join))
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

async fn require_join(State(store): State<Store>, req: Request, next: Next) -> Response {
    if let Err(err) = check_join(&store, req.headers()) {
        return err.into_response();
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

fn check_node_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > 253 {
        return Err(ApiError::bad_request(
            "node_name must be 1..=253 characters",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    {
        return Err(ApiError::bad_request(
            "node_name may contain only letters, digits, '-', '.' and '_'",
        ));
    }
    Ok(())
}

async fn hello(
    State(q): State<CommandQueue>,
    Json(h): Json<AgentHello>,
) -> ApiResult<Json<HelloAck>> {
    check_node_name(&h.node_name)?;
    Ok(Json(q.hello(&h)))
}

async fn heartbeat(
    State(q): State<CommandQueue>,
    Json(hb): Json<Heartbeat>,
) -> ApiResult<Json<HeartbeatAck>> {
    check_node_name(&hb.node_name)?;
    Ok(Json(q.heartbeat(&hb)))
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
    Query(p): Query<PollQuery>,
) -> ApiResult<Json<Vec<Command>>> {
    check_node_name(&node)?;
    let wait = Duration::from_secs(p.wait.unwrap_or(MAX_LONG_POLL_SECS).min(MAX_LONG_POLL_SECS));
    let max = p.max.unwrap_or(DEFAULT_POLL_BATCH).clamp(1, 256);
    Ok(Json(q.next(&node, wait, max).await))
}

async fn results(
    State(q): State<CommandQueue>,
    Path(node): Path<String>,
    Json(results): Json<Vec<CommandResult>>,
) -> ApiResult<Json<ReportAck>> {
    check_node_name(&node)?;
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

    use chrono::{Duration as ChronoDuration, Utc};
    use keep::{create_join_token, redeem_join_token, JoinToken};
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
    async fn unset_require_join_leaves_the_router_open() {
        if join_required() {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_store(CommandQueue::default(), Store::in_memory().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = AgentClient::new(format!("http://{addr}"), "n1").unwrap();
        client.hello(&hello("n1")).await.unwrap();
        server.abort();
    }
}
