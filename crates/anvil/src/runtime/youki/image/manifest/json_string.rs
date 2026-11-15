// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn json_string(
    value: &serde_json::Value,
    a: &str,
    b: &str,
) -> String {
    value
        .get(a)
        .or_else(|| value.get(b))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}
