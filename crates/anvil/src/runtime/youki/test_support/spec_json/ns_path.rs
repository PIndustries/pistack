// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn ns_path(cfg: &serde_json::Value, kind: &str) -> Option<String> {
    cfg["linux"]["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["type"] == kind)
        .and_then(|n| n.get("path"))
        .and_then(|p| p.as_str())
        .map(str::to_owned)
}
