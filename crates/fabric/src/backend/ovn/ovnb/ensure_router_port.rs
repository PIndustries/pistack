// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{map, set};
use super::super::{
    by_name::by_name, consts::GATEWAY_TAG, lrp_name::lrp_name, onto_router::onto_router,
    onto_switch::onto_switch, ovn_backend::OvnBackend, router_port::RouterPort,
    switch_name::switch_name,
};
use pistack_types::{ApiError, ApiResult, Id};
use serde_json::json;

impl OvnBackend {
    /// One router port and its `type=router` peer on the switch.
    pub(in crate::backend::ovn) async fn ensure_router_port(
        &self,
        rtr: &Id,
        router: &str,
        want: &RouterPort,
    ) -> ApiResult<()> {
        let port = &want.port;
        let mac = port.spec.mac.trim();
        if mac.is_empty() {
            return Err(ApiError::unavailable(format!(
                "router port {} has no mac yet",
                port.meta.id
            )));
        }
        let lrp = lrp_name(&port.meta.id);
        let mut ext = self.tags(rtr, port.meta.project.as_ref());
        ext.push(("pistack:port".into(), port.meta.id.to_string()));
        if want.gateway {
            ext.push((GATEWAY_TAG.into(), "true".into()));
        }
        let row = json!({
            "name": lrp,
            "mac": mac,
            "networks": set([json!(want.network)]),
            "external_ids": map(ext),
        });
        let lrp_uuid = self
            .ensure_row(
                "Logical_Router_Port",
                by_name(&lrp),
                row,
                onto_router(router, "ports"),
            )
            .await?;
        if want.gateway {
            self.set_gateway_chassis(&rtr.to_string(), &lrp, &lrp_uuid)
                .await?;
        }
        let name = port.meta.id.to_string();
        let mut ext = self.tags(&port.meta.id, port.meta.project.as_ref());
        ext.push(("pistack:device".into(), rtr.to_string()));
        let row = json!({
            "name": name,
            "type": "router",
            "addresses": "router",
            "options": map([("router-port", lrp.as_str())]),
            "external_ids": map(ext),
        });
        self.ensure_row(
            "Logical_Switch_Port",
            by_name(&name),
            row,
            onto_switch(&switch_name(&port.spec.network)),
        )
        .await?;
        Ok(())
    }
}
