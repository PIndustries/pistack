// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The edge path: find the edge host, queue one publish for the listener,
//! and start no process.

use super::kind::DoorSide;
use super::plan::DoorPlan;
use super::process::stop_process;
use pistack_rpc::CommandKind;
use pistack_store::Store;
use pistack_types::{kind, ApiError, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

pub(super) struct EdgePublish {
    key: String,
}

pub(super) fn edge_published() -> &'static parking_lot::Mutex<HashMap<String, EdgePublish>> {
    static EDGE: OnceLock<parking_lot::Mutex<HashMap<String, EdgePublish>>> = OnceLock::new();
    EDGE.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct HostSpecView {
    #[serde(default)]
    pub(super) node_name: String,
    #[serde(default)]
    pub(super) drivers: Vec<String>,
}

impl Spec for HostSpecView {
    const KIND: &'static str = kind::HOST;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct HostStatusView {}

pub(super) fn edge_node(store: &Store) -> Result<String, ApiError> {
    let hosts = store
        .collection::<Resource<HostSpecView, HostStatusView>>()
        .list(None)
        .map_err(|_| ApiError::unavailable("no edge host is registered"))?;
    let mut nodes: Vec<String> = hosts
        .into_iter()
        .filter(|host| host.spec.drivers.iter().any(|driver| driver == "edge"))
        .map(|host| host.spec.node_name)
        .filter(|name| !name.is_empty() && !name.chars().any(|c| c.is_whitespace()))
        .collect();
    nodes.sort();
    nodes.dedup();
    nodes
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::unavailable("no edge host is registered"))
}

pub(super) async fn apply_edge(
    id: &str,
    listen: &str,
    upstream: &str,
    mut plan: DoorPlan,
    side: DoorSide<'_>,
) -> Result<DoorPlan, ApiError> {
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
