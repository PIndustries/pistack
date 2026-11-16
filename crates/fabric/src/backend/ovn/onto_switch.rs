// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb;
use super::super::ovsdb::set;
use super::by_name::by_name;
use serde_json::{json, Value};

/// Operations that put the row inserted as `new` on switch `switch`, and
/// abort if that switch does not exist (an unreferenced switch port would
/// otherwise be dropped silently at commit).
pub(in crate::backend::ovn) fn onto_switch(switch: &str) -> Vec<Value> {
    vec![
        ovsdb::present("Logical_Switch", by_name(switch)),
        ovsdb::mutate(
            "Logical_Switch",
            by_name(switch),
            vec![json!(["ports", "insert", set([ovsdb::named("new")])])],
        ),
    ]
}
