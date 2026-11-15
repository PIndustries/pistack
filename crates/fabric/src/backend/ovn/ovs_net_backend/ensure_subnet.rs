// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{self, map};
use super::super::dhcp_options::dhcp_options;
use super::super::ovn_backend::OvnBackend;
use super::super::tagged::tagged;
use crate::types::{Network, Subnet};
use pistack_types::ApiResult;
use serde_json::json;

impl OvnBackend {
    /// `DHCP_Options` (one row per subnet with `enable_dhcp`):
    /// * `cidr = <spec.cidr>`
    /// * `options:server_id = <gateway or first host address>`,
    ///   `options:server_mac = <a stable locally administered mac>`,
    ///   `options:lease_time = 43200`, `options:router = <gateway>`,
    ///   `options:dns_server = {<dns_nameservers>}`,
    ///   `options:mtu = <network mtu>`,
    ///   `options:classless_static_route =
    ///   {169.254.169.254/32,0.0.0.0, <host_routes>, 0.0.0.0/0,<gw>}`:
    ///   metadata on-link, then the subnet's own routes, then the default
    ///   route when there is a gateway.
    /// * IPv6: `options:server_id` is the server mac, `options:dns_server`
    ///   as above; no router option.
    /// * `external_ids:pistack:subnet = <subnet id>`,
    ///   `external_ids:pistack:network = <net id>`.
    ///
    /// A subnet without DHCP has no row; turning DHCP off deletes it.
    /// Fabric allocates every address itself, so the switch does not get
    /// `other_config:subnet`.
    pub(in crate::backend::ovn::ovs_net_backend) async fn ensure_subnet_body(
        &self,
        net: &Network,
        subnet: &Subnet,
    ) -> ApiResult<()> {
        let id = &subnet.meta.id;
        let cond = tagged("pistack:subnet", id);
        if !subnet.spec.enable_dhcp {
            self.run(vec![ovsdb::delete("DHCP_Options", cond)]).await?;
            return Ok(());
        }
        let mut ext = self.tags(id, subnet.meta.project.as_ref());
        ext.push(("pistack:subnet".into(), id.to_string()));
        ext.push(("pistack:network".into(), net.meta.id.to_string()));
        let row = json!({
            "cidr": subnet.spec.cidr.trunc().to_string(),
            "options": map(dhcp_options(net, subnet)),
            "external_ids": map(ext),
        });
        self.ensure_row("DHCP_Options", cond, row, vec![]).await?;
        Ok(())
    }
}
