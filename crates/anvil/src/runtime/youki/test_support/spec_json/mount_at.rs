// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn mount_at<'a>(cfg: &'a serde_json::Value, dest: &str) -> &'a serde_json::Value {
    cfg["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["destination"] == dest)
        .unwrap()
}
