// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn ns_kinds(cfg: &serde_json::Value) -> Vec<&str> {
    cfg["linux"]["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|ns| ns["type"].as_str().unwrap())
        .collect()
}
