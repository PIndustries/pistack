// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Queuing one edge publish for a listener and starting no process.

use super::super::kind::DoorSide;
use super::super::plan::DoorPlan;
use super::super::process::stop_process;
use super::edge_node::edge_node;
use super::edge_publish::EdgePublish;
use super::edge_published::edge_published;
use pistack_rpc::CommandKind;
use pistack_types::{ApiError, ApiResult};

pub(crate) async fn apply_edge(
    id: &str,
    listen: &str,
    upstream: &str,
    mut plan: DoorPlan,
    side: DoorSide<'_>,
) -> ApiResult<DoorPlan> {
    let node = edge_node(side.store)?;
    let Some(queue) = side.queue else {
        return Err(ApiError::unavailable("edge publish was not queued"));
    };
    let key = format!("{node}\n{listen}\n{upstream}");
    if edge_published()
        .lock()
        .get(id)
        .is_some_and(|published| published.key == key)
    {
        plan.note = format!("publish queued for {node}");
        return Ok(plan);
    }
    let payload = serde_json::json!({
        "listener": id,
        "listen": listen,
        "upstream": upstream,
    });
    let ticket = queue
        .enqueue(&node, CommandKind::Publish, payload)
        .map_err(|_| ApiError::unavailable("edge publish was not queued"))?;
    let _ticket = ticket;
    stop_process(id);
    edge_published()
        .lock()
        .insert(id.to_owned(), EdgePublish { key });
    plan.note = format!("publish queued for {node}");
    Ok(plan)
}
