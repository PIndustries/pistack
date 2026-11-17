// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Choosing the edge host to publish on: the first registered host whose
//! drivers include `edge`, by node name.

use super::host_spec_view::HostSpecView;
use super::host_status_view::HostStatusView;
use pistack_store::Store;
use pistack_types::{ApiError, Resource};

pub(crate) fn edge_node(store: &Store) -> Result<String, ApiError> {
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
