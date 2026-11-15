// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{self, map, set};
use super::super::address_entry::address_entry;
use super::super::by_name::by_name;
use super::super::consts::DROP_GROUP;
use super::super::group_name::group_name;
use super::super::lsp_name::lsp_name;
use super::super::onto_switch::onto_switch;
use super::super::ovn_backend::OvnBackend;
use super::super::pair_rows::pair_rows;
use super::super::pick_dhcp::pick_dhcp;
use super::super::pub_consts::INTEGRATION_BRIDGE;
use super::super::switch_name::switch_name;
use super::super::tagged::tagged;
use crate::types::{Network, Port, PortBinding, SecurityGroup};
use pistack_types::ApiResult;
use serde_json::{json, Value};
use std::net::IpAddr;

impl OvnBackend {
    /// `Logical_Switch_Port` on `ps-<net id>`:
    /// * `name = <port uuid>`, `type = ""` (VIF) for machine ports. Router
    ///   ports are written with their router (see [`Self::ensure_router`]);
    ///   here they only get a binding.
    /// * `addresses = ["<mac> <ip1> <ip2>..."]`, or `["unknown"]` before a
    ///   mac is assigned. An allowed address pair joins it only as an
    ///   address it can name: a single address bare, `"<pair mac or the port
    ///   mac> <address>"`; a wider prefix never, since the column takes plain
    ///   addresses, and then only its MAC, `"<pair mac>"`, and only when that
    ///   mac is not the port's own — that is all the switch needs to know
    ///   where the mac lives.
    /// * `port_security` holds every entry `addresses` would carry plus each
    ///   pair with its mask, `"<pair mac or the port mac> <pair prefix>"`,
    ///   when `spec.port_security`, so a pair passes anti-spoofing, else `[]`.
    /// * `dhcpv4_options` / `dhcpv6_options` = the network's `DHCP_Options`
    ///   row whose range holds one of the port's addresses.
    /// * `external_ids:pistack:id`, `pistack:device = <device id>`,
    ///   `pistack:project`, `pistack:term`.
    /// * Security groups: with `port_security` the port joins
    ///   `pg_pistack_drop` and each of its groups' `Port_Group`, and leaves
    ///   any other PiStack group; without it the port is in no group and
    ///   gets no ACLs (OVN default allow).
    ///
    /// Binding: the agent sets `external_ids:iface-id = <port uuid>` on the
    /// OVS interface on the host. The returned `PortBinding` is
    /// `{vif_type: "ovs", vif_details: {bridge: "br-int", iface_id: <port uuid>}}`.
    pub(in crate::backend::ovn::ovs_net_backend) async fn ensure_port_body(
        &self,
        net: &Network,
        port: &Port,
        groups: &[SecurityGroup],
    ) -> ApiResult<PortBinding> {
        let id = &port.meta.id;
        if port.is_router_port() {
            return Ok(PortBinding {
                vif_type: "ovs".into(),
                vif_details: Default::default(),
            });
        }
        let ips = &port.spec.fixed_ips;
        let mac = port.spec.mac.trim();
        let (addresses, security) = if mac.is_empty() {
            (json!("unknown"), Vec::new())
        } else {
            let rows = pair_rows(port);
            let own = address_entry(mac, ips.iter().map(IpAddr::to_string));
            let addresses: Vec<Value> = std::iter::once(own.clone())
                .chain(rows.iter().filter_map(|r| r.addresses.clone()))
                .map(|e| json!(e))
                .collect();
            let security = if port.spec.port_security {
                std::iter::once(own)
                    .chain(rows.into_iter().map(|r| r.security))
                    .map(|e| json!(e))
                    .collect()
            } else {
                Vec::new()
            };
            (set(addresses), security)
        };
        let dhcp = self
            .run(vec![ovsdb::select(
                "DHCP_Options",
                tagged("pistack:network", &net.meta.id),
            )])
            .await?;
        let dhcp = ovsdb::rows(&dhcp[0]);
        let mut ext = self.tags(id, port.meta.project.as_ref());
        if let Some(d) = &port.spec.device {
            ext.push(("pistack:device".into(), d.to_string()));
        }
        let name = lsp_name(id);
        // A port written before the name lost its prefix.
        self.drop_switch_ports(by_name(&id.to_string())).await?;
        let row = json!({
            "name": name,
            "addresses": addresses,
            "port_security": set(security),
            "dhcpv4_options": set(pick_dhcp(&dhcp, ips, true)),
            "dhcpv6_options": set(pick_dhcp(&dhcp, ips, false)),
            "external_ids": map(ext),
        });
        let switch = switch_name(&net.meta.id);
        let lsp = self
            .ensure_row(
                "Logical_Switch_Port",
                by_name(&name),
                row,
                onto_switch(&switch),
            )
            .await?;
        let mut want = Vec::new();
        if port.spec.port_security && !mac.is_empty() {
            self.ensure_drop_group().await?;
            want.push(DROP_GROUP.to_string());
            for sg in groups {
                self.ensure_sg_group(sg, None).await?;
                want.push(group_name(&sg.meta.id));
            }
        }
        self.set_membership(&lsp, &want).await?;
        Ok(PortBinding {
            vif_type: "ovs".into(),
            vif_details: [
                ("bridge".to_string(), INTEGRATION_BRIDGE.to_string()),
                ("iface_id".to_string(), name),
            ]
            .into_iter()
            .collect(),
        })
    }
}
