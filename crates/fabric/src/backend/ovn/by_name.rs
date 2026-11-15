// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb::eq;
use serde_json::{json, Value};

pub(in crate::backend::ovn) fn by_name(name: &str) -> Vec<Value> {
    vec![eq("name", json!(name))]
}
