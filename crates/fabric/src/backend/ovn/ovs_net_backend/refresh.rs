// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{self, includes, map};
use super::super::consts::GATEWAY_TAG;
use super::super::ext_value::ext_value;
use super::super::ovn_backend::OvnBackend;
use pistack_types::ApiResult;

impl OvnBackend {
    /// Schedule every gateway port again from the current chassis list.
    /// Nothing is written for a port whose chassis are already right.
    pub(in crate::backend::ovn::ovs_net_backend) async fn refresh_body(&self) -> ApiResult<()> {
        if !self.gateway_chassis.is_empty() {
            return Ok(());
        }
        let found = self
            .run(vec![ovsdb::select(
                "Logical_Router_Port",
                vec![includes("external_ids", map([(GATEWAY_TAG, "true")]))],
            )])
            .await?;
        for row in ovsdb::rows(&found[0]) {
            let (Some(u), Some(rtr), Some(name)) = (
                ovsdb::row_uuid(&row),
                ext_value(&row, "pistack:id"),
                row["name"].as_str(),
            ) else {
                continue;
            };
            self.set_gateway_chassis(&rtr, name, &u).await?;
        }
        Ok(())
    }
}
