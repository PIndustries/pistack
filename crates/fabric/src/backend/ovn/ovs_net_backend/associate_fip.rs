// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{self, map, set};
use super::super::super::FipView;
use super::super::lsp_name::lsp_name;
use super::super::onto_router::onto_router;
use super::super::ovn_backend::OvnBackend;
use super::super::router_name::router_name;
use super::super::tagged::tagged;
use super::super::unhook_nat::unhook_nat;
use crate::types::Network;
use pistack_types::{ApiError, ApiResult, Id};
use serde_json::json;

impl OvnBackend {
    /// `NAT` row on the router that has an interface on the target port's
    /// subnet (`view.router`):
    /// * `type = dnat_and_snat`, `external_ip = <fip address>`,
    ///   `logical_ip = <target fixed ip>`, `logical_port = <port uuid>`,
    ///   `external_mac = <target port mac>` (distributed: the chassis
    ///   hosting the machine answers ARP and forwards directly; a port
    ///   without a mac yet gets neither column and goes through the
    ///   gateway), `options:stateless = false`,
    ///   `external_ids:pistack:id = <fip id>`, `pistack:router = <rtr id>`.
    /// * A matching row writes nothing. A changed target or router replaces
    ///   the row in one transaction.
    /// * Disassociate (`target: None`): remove the NAT row matching
    ///   `external_ids:pistack:id`.
    /// * No router joining the port's subnet to the floating IP's network
    ///   is an `unavailable` error, retried by the reconciler. The message
    ///   names the port and both networks and says what fixes it: attach
    ///   the port's subnet to a router with that external network.
    /// * The address itself is reserved in the external subnet by fabric's
    ///   IPAM; OVN needs nothing for an unassociated FIP.
    pub(in crate::backend::ovn::ovs_net_backend) async fn associate_fip_body(
        &self,
        view: &FipView,
    ) -> ApiResult<()> {
        let fip = &view.fip;
        let id = &fip.meta.id;
        let Some((port, fixed)) = &view.target else {
            return self.delete_fip_body(id).await;
        };
        let address = fip
            .spec
            .address
            .ok_or_else(|| ApiError::unavailable(format!("floating ip {id} has no address yet")))?;
        let router = view.router.as_ref().ok_or_else(|| {
            let (port_net, fip_net) = match self.attached.get() {
                Some(a) => {
                    let col = a.store.collection::<Network>();
                    let named = |id: &Id| match col.try_get(id) {
                        Ok(Some(n)) => format!("{} [{id}]", n.meta.name),
                        _ => id.to_string(),
                    };
                    (named(&port.spec.network), named(&fip.spec.network))
                }
                None => (port.spec.network.to_string(), fip.spec.network.to_string()),
            };
            ApiError::unavailable(format!(
                "no router connects the network {port_net} of port {} to the floating ip's external network {fip_net}. Attach the port's subnet to a router with {fip_net} as its external network",
                port.meta.id
            ))
        })?;
        let mut ext = self.tags(id, fip.meta.project.as_ref());
        ext.push(("pistack:router".into(), router.meta.id.to_string()));
        let mac = port.spec.mac.trim();
        let (logical_port, external_mac) = if mac.is_empty() {
            (set([]), set([]))
        } else {
            (set([json!(lsp_name(&port.meta.id))]), set([json!(mac)]))
        };
        let row = json!({
            "type": "dnat_and_snat",
            "external_ip": address.to_string(),
            "logical_ip": fixed.to_string(),
            "logical_port": logical_port,
            "external_mac": external_mac,
            "options": map([("stateless", "false")]),
            "external_ids": map(ext),
        });
        let found = self
            .run(vec![ovsdb::select("NAT", tagged("pistack:id", id))])
            .await?;
        let have = ovsdb::rows(&found[0]);
        if let [one] = have.as_slice() {
            if ovsdb::row_matches(one, &row) {
                return Ok(());
            }
        }
        let mut ops = unhook_nat(&have);
        ops.push(ovsdb::insert("NAT", row, "new"));
        ops.extend(onto_router(&router_name(&router.meta.id), "nat"));
        self.run(ops).await?;
        Ok(())
    }
}
