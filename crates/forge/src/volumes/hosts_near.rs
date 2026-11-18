// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use crate::types::Host;

use super::held::Held;

/// The hosts that can attach every volume in `held`.
///
/// `None` when no volume is tied to a node, so the caller keeps its own
/// list. An error when the volumes are on different nodes, or when the
/// node that holds them is not among `hosts`.
pub fn hosts_near(held: &[Held], hosts: &[Host]) -> ApiResult<Option<Vec<Host>>> {
    let Some(first) = held.first() else {
        return Ok(None);
    };
    if let Some(other) = held.iter().find(|h| h.node != first.node) {
        return Err(ApiError::unavailable(format!(
            "no host can attach both volume {} (held on node {}) and volume {} (held on node {})",
            first.volume, first.node, other.volume, other.node
        )));
    }
    let near: Vec<Host> = hosts
        .iter()
        .filter(|h| h.spec.node_name == first.node)
        .cloned()
        .collect();
    if near.is_empty() {
        return Err(ApiError::unavailable(format!(
            "no host can attach volume {}: it is held on node {}, which is not one of the {} hosts this machine can use",
            first.volume,
            first.node,
            hosts.len()
        ))
        .with_detail("home_node", first.node.clone())
        .with_detail("hosts", hosts.len() as u64));
    }
    Ok(Some(near))
}
