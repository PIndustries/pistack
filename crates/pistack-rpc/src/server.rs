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
//! Authentication (mTLS or a join token) is the gateway's layer, not this
//! router's.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;

use crate::proto::{
    AgentHello, Command, CommandResult, Heartbeat, HeartbeatAck, HelloAck, ReportAck,
    DEFAULT_POLL_BATCH, MAX_LONG_POLL_SECS,
};
use crate::queue::CommandQueue;

/// Build the agent-facing router over `queue`.
pub fn router(queue: CommandQueue) -> Router {
    Router::new()
        .route("/agents/hello", post(hello))
        .route("/agents/heartbeat", post(heartbeat))
        .route("/agents/{node}/commands", get(commands))
        .route("/agents/{node}/results", post(results))
        .with_state(queue)
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
