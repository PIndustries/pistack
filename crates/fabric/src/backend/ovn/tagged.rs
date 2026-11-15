// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb::{includes, map};
use pistack_types::Id;
use serde_json::Value;

pub(in crate::backend::ovn) fn tagged(key: &str, id: &Id) -> Vec<Value> {
    vec![includes("external_ids", map([(key, id.to_string())]))]
}
