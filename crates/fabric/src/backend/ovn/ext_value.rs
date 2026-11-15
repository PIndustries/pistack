// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde_json::Value;

pub(in crate::backend::ovn) fn ext_value(row: &Value, key: &str) -> Option<String> {
    row["external_ids"][1]
        .as_array()?
        .iter()
        .find(|p| p[0] == key)
        .and_then(|p| p[1].as_str().map(str::to_string))
}
