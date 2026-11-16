// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn json_strings(
    value: &serde_json::Value,
    a: &str,
    b: &str,
) -> Vec<String> {
    match value.get(a).or_else(|| value.get(b)) {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}
