// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb;
use serde_json::Value;

pub(in crate::backend::ovn) fn uuid_strings(v: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = ovsdb::elems(v)
        .iter()
        .filter_map(|e| e.get(1).and_then(Value::as_str).map(str::to_string))
        .collect();
    out.sort();
    out
}
