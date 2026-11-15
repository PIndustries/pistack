// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde_json::Value;

pub(in crate::backend::ovn) fn ext_has(row: &Value, key: &str) -> bool {
    row["external_ids"][1]
        .as_array()
        .is_some_and(|ps| ps.iter().any(|p| p[0] == key))
}
