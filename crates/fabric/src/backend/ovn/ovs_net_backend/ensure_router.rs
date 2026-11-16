// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb::{self, eq, includes, map, set, uuid};
use super::super::super::RouterView;
use super::super::by_name::by_name;
use super::super::ext_value::ext_value;
use super::super::lrp_name::lrp_name;
use super::super::ovn_backend::OvnBackend;
use super::super::port_network::port_network;
use super::super::router_name::router_name;
use super::super::router_port::RouterPort;
use super::super::tagged::tagged;
use pistack_types::{ApiError, ApiResult};
use serde_json::json;
use std::collections::BTreeSet;
use std::net::IpAddr;

impl OvnBackend {
    /// `Logical_Router`:
    /// * `name = pr-<rtr id>`, `options:always_learn_from_arp_request=false`,
    ///   `options:dynamic_neigh_routers=true`.
    /// * Per interface subnet: `Logical_Router_Port`
    ///   `name = lrp-<port id>`, `mac = <port mac>`,
    ///   `networks = ["<gateway>/<prefix>"]`, linked from the
    ///   `Logical_Switch_Port` `type=router`,
    ///   `options:router-port=lrp-<port id>`, `addresses=["router"]`.
    /// * Gateway: an LRP on the external switch the same way, with
    ///   `networks = ["<gateway port ip>/<external subnet prefix>"]`, plus
    ///   `Gateway_Chassis` rows `name = lrp-<port id>_<chassis>` (one per
    ///   chassis in `PISTACK_OVN_GATEWAY_CHASSIS`, priority descending) so
    ///   the port is scheduled; a default `Logical_Router_Static_Route`
    ///   `ip_prefix=0.0.0.0/0` (`::/0` for IPv6),
    ///   `nexthop=<external subnet gateway>`; and a `NAT` row per IPv4
    ///   interface subnet, `type=snat`, `logical_ip=<interface cidr>`,
    ///   `external_ip=<gateway port fixed ip>`, so tenant traffic can leave.
    /// * `spec.routes` become `Logical_Router_Static_Route` rows
    ///   (`ip_prefix`, `nexthop`).
    /// * LRPs, routes, router NAT rows and gateway chassis carry
    ///   `external_ids:pistack:id = <rtr id>` (LRPs also `pistack:port`).
    ///   Each ensure compares them with the view: LRPs no longer wanted go
    ///   with their switch peers, and the route and router NAT sets are
    ///   replaced only when they differ. Floating IP NAT rows on the router
    ///   carry their own id and are left alone.
    pub(in crate::backend::ovn::ovs_net_backend) async fn ensure_router_body(
        &self,
        view: &RouterView,
    ) -> ApiResult<()> {
        let r = &view.router;
        let id = &r.meta.id;
        let name = router_name(id);
        let mut ports = Vec::new();
        for (subnet, port) in &view.interfaces {
            ports.push(RouterPort {
                port: port.clone(),
                network: port_network(port, subnet)?,
                gateway: false,
            });
        }
        let mut external_ip = None;
        let mut nexthop = None;
        if let Some((net, port)) = &view.gateway {
            let subnet = view.gateway_subnet.as_ref().ok_or_else(|| {
                ApiError::unavailable(format!(
                    "router {id} gateway port has no subnet on network {}",
                    net.meta.id
                ))
            })?;
            let network = port_network(port, subnet)?;
            external_ip = port
                .spec
                .fixed_ips
                .iter()
                .copied()
                .find(|ip| subnet.spec.cidr.contains(ip));
            nexthop = subnet.spec.gateway;
            ports.push(RouterPort {
                port: port.clone(),
                network,
                gateway: true,
            });
        }

        let row = json!({
            "name": name,
            "options": map([
                ("always_learn_from_arp_request", "false"),
                ("dynamic_neigh_routers", "true"),
            ]),
            "external_ids": map(self.tags(id, r.meta.project.as_ref())),
        });
        self.ensure_row("Logical_Router", by_name(&name), row, vec![])
            .await?;
        for p in &ports {
            self.ensure_router_port(id, &name, p).await?;
        }

        // Router ports no longer in the view go, with their switch peers.
        let keep: BTreeSet<String> = ports.iter().map(|p| lrp_name(&p.port.meta.id)).collect();
        let found = self
            .run(vec![ovsdb::select(
                "Logical_Router_Port",
                tagged("pistack:id", id),
            )])
            .await?;
        for row in ovsdb::rows(&found[0]) {
            if row["name"].as_str().is_some_and(|n| keep.contains(n)) {
                continue;
            }
            let Some(u) = ovsdb::row_uuid(&row) else {
                continue;
            };
            self.run(vec![
                ovsdb::mutate(
                    "Logical_Router",
                    vec![includes("ports", uuid(&u))],
                    vec![json!(["ports", "delete", set([uuid(&u)])])],
                ),
                ovsdb::delete("Logical_Router_Port", vec![eq("_uuid", uuid(&u))]),
            ])
            .await?;
            if let Some(peer) = ext_value(&row, "pistack:port") {
                let mut cond = by_name(&peer);
                cond.push(eq("type", json!("router")));
                self.drop_switch_ports(cond).await?;
            }
        }

        let tags = map(self.tags(id, r.meta.project.as_ref()));
        let mut routes = Vec::new();
        if let Some(nh) = nexthop {
            let any = if nh.is_ipv4() { "0.0.0.0/0" } else { "::/0" };
            routes.push(json!({
                "ip_prefix": any,
                "nexthop": nh.to_string(),
                "external_ids": tags.clone(),
            }));
        }
        for sr in &r.spec.routes {
            routes.push(json!({
                "ip_prefix": sr.destination.trunc().to_string(),
                "nexthop": sr.nexthop.to_string(),
                "external_ids": tags.clone(),
            }));
        }
        self.ensure_router_set(
            id,
            &name,
            "Logical_Router_Static_Route",
            "static_routes",
            routes,
            &["ip_prefix", "nexthop"],
        )
        .await?;

        let mut snat = Vec::new();
        if let Some(IpAddr::V4(ext)) = external_ip {
            for (subnet, _) in &view.interfaces {
                if subnet.spec.cidr.addr().is_ipv4() {
                    snat.push(json!({
                        "type": "snat",
                        "logical_ip": subnet.spec.cidr.trunc().to_string(),
                        "external_ip": ext.to_string(),
                        "external_ids": tags.clone(),
                    }));
                }
            }
        }
        self.ensure_router_set(
            id,
            &name,
            "NAT",
            "nat",
            snat,
            &["type", "logical_ip", "external_ip"],
        )
        .await
    }
}
