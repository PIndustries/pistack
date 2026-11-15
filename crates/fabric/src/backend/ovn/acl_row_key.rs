// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde_json::Value;

pub(in crate::backend::ovn) fn acl_row_key(row: &Value) -> (String, i64, String, String) {
    let s = |c: &str| row.get(c).and_then(Value::as_str).unwrap_or("").to_string();
    (
        s("direction"),
        row.get("priority").and_then(Value::as_i64).unwrap_or(-1),
        s("match"),
        s("action"),
    )
}
