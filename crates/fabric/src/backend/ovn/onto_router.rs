// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb;
use super::super::ovsdb::set;
use super::by_name::by_name;
use serde_json::{json, Value};

/// Operations that put the row inserted as `new` into router `router`'s
/// column `col`, and abort if that router does not exist.
pub(in crate::backend::ovn) fn onto_router(router: &str, col: &str) -> Vec<Value> {
    vec![
        ovsdb::present("Logical_Router", by_name(router)),
        ovsdb::mutate(
            "Logical_Router",
            by_name(router),
            vec![json!([col, "insert", set([ovsdb::named("new")])])],
        ),
    ]
}
