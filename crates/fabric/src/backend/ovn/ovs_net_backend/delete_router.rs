// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{self, eq, includes, map};
use super::super::by_name::by_name;
use super::super::ovn_backend::OvnBackend;
use super::super::router_name::router_name;
use pistack_types::{ApiResult, Id};
use serde_json::json;

impl OvnBackend {
    /// Delete `pr-<rtr id>`; its LRPs, NAT rows, routes and gateway chassis
    /// go with it (none of those tables is a root table). Then take the
    /// router's `type=router` ports off their switches. A second delete
    /// succeeds.
    pub(in crate::backend::ovn::ovs_net_backend) async fn delete_router_body(
        &self,
        id: &Id,
    ) -> ApiResult<()> {
        self.run(vec![ovsdb::delete(
            "Logical_Router",
            by_name(&router_name(id)),
        )])
        .await?;
        self.drop_switch_ports(vec![
            includes("external_ids", map([("pistack:device", id.to_string())])),
            eq("type", json!("router")),
        ])
        .await
    }
}
