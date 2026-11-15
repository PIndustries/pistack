// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb;
use super::super::ovsdb::{eq, includes, set, uuid};
use serde_json::{json, Value};

/// Operations that take each NAT row off whatever router holds it and
/// delete it.
pub(in crate::backend::ovn) fn unhook_nat(rows: &[Value]) -> Vec<Value> {
    let mut ops = Vec::new();
    for row in rows {
        let Some(u) = ovsdb::row_uuid(row) else {
            continue;
        };
        ops.push(ovsdb::mutate(
            "Logical_Router",
            vec![includes("nat", uuid(&u))],
            vec![json!(["nat", "delete", set([uuid(&u)])])],
        ));
        ops.push(ovsdb::delete("NAT", vec![eq("_uuid", uuid(&u))]));
    }
    ops
}
