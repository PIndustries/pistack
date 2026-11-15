// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::super::ovsdb::{eq, includes, set, uuid};
use super::super::{by_name::by_name, ext_has::ext_has, ovn_backend::OvnBackend};
use pistack_types::ApiResult;
use serde_json::{json, Value};

impl OvnBackend {
    /// Put switch port `lsp` in exactly the PiStack groups named `want`.
    /// Groups without `pistack:id` are never touched.
    pub(in crate::backend::ovn) async fn set_membership(
        &self,
        lsp: &str,
        want: &[String],
    ) -> ApiResult<()> {
        let cur = self
            .run(vec![ovsdb::select(
                "Port_Group",
                vec![includes("ports", uuid(lsp))],
            )])
            .await?;
        let cur = ovsdb::rows(&cur[0]);
        let name_of = |r: &Value| {
            r.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let have: Vec<String> = cur.iter().map(name_of).collect();
        let mut ops = Vec::new();
        for w in want.iter().filter(|w| !have.contains(w)) {
            ops.push(ovsdb::present("Port_Group", by_name(w)));
            ops.push(ovsdb::mutate(
                "Port_Group",
                by_name(w),
                vec![json!(["ports", "insert", set([uuid(lsp)])])],
            ));
        }
        for r in cur
            .iter()
            .filter(|r| ext_has(r, "pistack:id") && !want.contains(&name_of(r)))
        {
            if let Some(u) = ovsdb::row_uuid(r) {
                ops.push(ovsdb::mutate(
                    "Port_Group",
                    vec![eq("_uuid", uuid(&u))],
                    vec![json!(["ports", "delete", set([uuid(lsp)])])],
                ));
            }
        }
        if !ops.is_empty() {
            self.run(ops).await?;
        }
        Ok(())
    }
}
