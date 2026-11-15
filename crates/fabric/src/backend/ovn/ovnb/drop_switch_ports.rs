// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::super::ovsdb::{eq, includes, set, uuid};
use super::super::ovn_backend::OvnBackend;
use pistack_types::ApiResult;
use serde_json::{json, Value};

impl OvnBackend {
    /// Take every switch port matching `cond` off its switch and delete it.
    pub(in crate::backend::ovn) async fn drop_switch_ports(
        &self,
        cond: Vec<Value>,
    ) -> ApiResult<()> {
        let found = self
            .run(vec![ovsdb::select("Logical_Switch_Port", cond)])
            .await?;
        let mut ops = Vec::new();
        for row in ovsdb::rows(&found[0]) {
            let Some(u) = ovsdb::row_uuid(&row) else {
                continue;
            };
            ops.push(ovsdb::mutate(
                "Logical_Switch",
                vec![includes("ports", uuid(&u))],
                vec![json!(["ports", "delete", set([uuid(&u)])])],
            ));
            ops.push(ovsdb::delete(
                "Logical_Switch_Port",
                vec![eq("_uuid", uuid(&u))],
            ));
        }
        if !ops.is_empty() {
            self.run(ops).await?;
        }
        Ok(())
    }
}
