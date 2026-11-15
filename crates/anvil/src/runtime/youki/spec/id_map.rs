// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// One map entry: container id 0 is the agent's id, size 1. youki writes that
/// directly and does not look up `newuidmap`.
pub(in crate::runtime::youki) fn id_map(host_id: u32) -> serde_json::Value {
    serde_json::json!([{ "containerID": 0, "hostID": host_id, "size": 1 }])
}
