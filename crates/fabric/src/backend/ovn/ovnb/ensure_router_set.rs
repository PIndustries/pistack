// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::super::ovsdb::{eq, set, uuid};
use super::super::{by_name::by_name, ovn_backend::OvnBackend, tagged::tagged};
use pistack_types::{ApiResult, Id};
use serde_json::{json, Value};

impl OvnBackend {
    /// Make the rows of `table` that router `rtr` owns (tagged with its id,
    /// held in router column `col`) the set `want`. Rows are the same when
    /// the `key` columns match; any difference replaces the whole set in one
    /// transaction, and a match writes nothing.
    pub(in crate::backend::ovn) async fn ensure_router_set(
        &self,
        rtr: &Id,
        router: &str,
        table: &str,
        col: &str,
        want: Vec<Value>,
        key: &[&str],
    ) -> ApiResult<()> {
        let found = self
            .run(vec![ovsdb::select(table, tagged("pistack:id", rtr))])
            .await?;
        let have = ovsdb::rows(&found[0]);
        let keys = |rows: &[Value]| {
            let mut out: Vec<String> = rows
                .iter()
                .map(|r| {
                    let k: Vec<Value> = key
                        .iter()
                        .map(|c| ovsdb::canon(r.get(*c).unwrap_or(&json!(["set", []]))))
                        .collect();
                    Value::Array(k).to_string()
                })
                .collect();
            out.sort();
            out
        };
        if keys(&have) == keys(&want) {
            return Ok(());
        }
        let mut ops = vec![ovsdb::present("Logical_Router", by_name(router))];
        let old: Vec<Value> = have
            .iter()
            .filter_map(ovsdb::row_uuid)
            .map(|u| uuid(&u))
            .collect();
        if !old.is_empty() {
            ops.push(ovsdb::mutate(
                "Logical_Router",
                by_name(router),
                vec![json!([col, "delete", set(old.clone())])],
            ));
            for u in old {
                ops.push(ovsdb::delete(table, vec![eq("_uuid", u)]));
            }
        }
        let mut names = Vec::new();
        for (i, row) in want.into_iter().enumerate() {
            let n = format!("row{i}");
            ops.push(ovsdb::insert(table, row, &n));
            names.push(ovsdb::named(&n));
        }
        if !names.is_empty() {
            ops.push(ovsdb::mutate(
                "Logical_Router",
                by_name(router),
                vec![json!([col, "insert", set(names)])],
            ));
        }
        self.run(ops).await?;
        Ok(())
    }
}
