// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! OVN northbound backend. Networks, subnets and machine ports are written
//! to OVN's northbound database; routers, security groups and floating IPs
//! still return `not_implemented` and land in follow-ups. The doc comment
//! on each of those is the contract for the real thing.
//!
//! The PiStack store stays the record. OVN holds a copy the leader writes:
//! * Every row fabric creates carries `external_ids:pistack:id=<our id>`,
//!   `pistack:project` when the resource has one, and `pistack:term`, the
//!   cluster term of the leader that last wrote it. Rows without
//!   `pistack:id` are never touched.
//! * Each ensure reads the row first and writes only when it differs, so a
//!   repeat is a no-op. Inserts are guarded so a twin written since the
//!   read aborts the transaction instead of duplicating the row.
//! * Only the leader writes. Each transaction checks leadership first and
//!   is abandoned if leadership drops while it waits for the database. A
//!   transaction already sent may still commit; the new leader's full pass
//!   on promotion rewrites anything that differs from the store.
//! * Names: `Logical_Switch.name = ps-<net id>`, switch ports are named by
//!   the port id, a provider network's uplink is `provnet-<net id>`.
//!   Later: `Logical_Router.name = pr-<rtr id>`,
//!   `Port_Group.name = pg_<sg uuid with '-' replaced by '_'>` (OVN names
//!   must be valid identifiers), `Address_Set` likewise with `as_` prefix.
//!
//! On each host, OVS and `ovn-controller` realise the switches. The agent
//! plugs a machine's tap or a pod's link into the integration bridge with
//! `external_ids:iface-id=<port id>`; the port binding below tells it which
//! bridge and id to use.

use super::ovsdb::{self, eq, includes, map, set, uuid, Transport};
use super::{FipView, NetBackend, RouterView};
use crate::types::{Network, NetworkKind, Port, PortBinding, SecurityGroup, Subnet};
use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id, Leadership};
use serde_json::{json, Value};
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

const DB: &str = "OVN_Northbound";

/// Environment variable naming the northbound database endpoint.
pub const NB_ENV: &str = "PISTACK_OVN_NB";
/// Where a host running the northbound database listens by default.
pub const DEFAULT_NB: &str = "unix:/var/run/ovn/ovnnb_db.sock";
/// The OVS bridge `ovn-controller` manages on every host.
pub const INTEGRATION_BRIDGE: &str = "br-int";
/// The cluster term, as the cluster saves it in the store.
const TERM_KEY: &str = "cluster_term";

struct Attached {
    leadership: Leadership,
    store: Store,
}

/// Writes OVN's northbound database.
pub struct OvnBackend {
    /// e.g. `unix:/var/run/ovn/ovnnb_db.sock` or `tcp:10.0.0.11:6641`.
    pub nb_endpoint: String,
    nb: Arc<dyn Transport>,
    attached: OnceLock<Attached>,
}

impl std::fmt::Debug for OvnBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OvnBackend")
            .field("nb_endpoint", &self.nb_endpoint)
            .finish()
    }
}

/// Stands in for a client when the endpoint does not parse, so the error
/// shows on every resource instead of stopping the server.
struct BadEndpoint(String);

#[async_trait]
impl Transport for BadEndpoint {
    async fn transact(&self, _db: &str, _ops: Vec<Value>) -> ApiResult<Vec<Value>> {
        Err(ApiError::unavailable(self.0.clone()))
    }
}

impl OvnBackend {
    pub fn new(nb_endpoint: impl Into<String>) -> Self {
        let nb_endpoint = nb_endpoint.into();
        let nb: Arc<dyn Transport> = match ovsdb::Endpoint::parse(&nb_endpoint) {
            Ok(ep) => Arc::new(ovsdb::Client::new(ep)),
            Err(e) => Arc::new(BadEndpoint(format!("{NB_ENV}: {}", e.message))),
        };
        Self::with_transport(nb_endpoint, nb)
    }

    /// Endpoint from `PISTACK_OVN_NB`, else [`DEFAULT_NB`].
    pub fn from_env() -> Self {
        let ep = std::env::var(NB_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_NB.to_string());
        Self::new(ep)
    }

    pub fn with_transport(nb_endpoint: impl Into<String>, nb: Arc<dyn Transport>) -> Self {
        Self {
            nb_endpoint: nb_endpoint.into(),
            nb,
            attached: OnceLock::new(),
        }
    }

    fn todo(&self, op: &str) -> ApiError {
        ApiError::not_implemented(format!(
            "ovn backend: {op} not implemented (nb={})",
            self.nb_endpoint
        ))
        .with_detail("op", op)
    }

    fn term(&self) -> u64 {
        self.attached
            .get()
            .and_then(|a| a.store.get_meta::<u64>(TERM_KEY).ok().flatten())
            .unwrap_or(0)
    }

    /// Our `external_ids` for a row standing for `id`.
    fn tags(&self, id: &Id, project: Option<&Id>) -> Vec<(String, String)> {
        let mut t = vec![("pistack:id".to_string(), id.to_string())];
        if let Some(p) = project {
            t.push(("pistack:project".into(), p.to_string()));
        }
        t.push(("pistack:term".into(), self.term().to_string()));
        t
    }

    /// One transaction, only while leader, abandoned if leadership drops.
    async fn run(&self, ops: Vec<Value>) -> ApiResult<Vec<Value>> {
        let Some(a) = self.attached.get() else {
            return self.nb.transact(DB, ops).await;
        };
        if !a.leadership.is_leader() {
            return Err(ApiError::unavailable("ovn: not leader; skipping write"));
        }
        let mut rx = a.leadership.subscribe();
        let lost = async move {
            loop {
                if rx.changed().await.is_err() || !*rx.borrow() {
                    return;
                }
            }
        };
        tokio::select! {
            r = self.nb.transact(DB, ops) => r,
            _ = lost => Err(ApiError::unavailable("ovn: lost leadership; write abandoned")),
        }
    }

    /// Make the one row matching `cond` look like `row`: insert it (plus
    /// `extra`, which may refer to it as `new`) when missing, update it
    /// when it differs, leave it alone when it matches. Returns its uuid.
    async fn ensure_row(
        &self,
        table: &str,
        cond: Vec<Value>,
        row: Value,
        extra: Vec<Value>,
    ) -> ApiResult<String> {
        let found = self.run(vec![ovsdb::select(table, cond.clone())]).await?;
        let found = ovsdb::rows(&found[0]);
        match found.as_slice() {
            [] => {
                let mut ops = vec![ovsdb::absent(table, cond), ovsdb::insert(table, row, "new")];
                ops.extend(extra);
                let out = self.run(ops).await?;
                out[1]["uuid"][1]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| ApiError::unavailable(format!("ovn: {table} insert: no uuid")))
            }
            [one] => {
                let u = ovsdb::row_uuid(one)
                    .ok_or_else(|| ApiError::unavailable(format!("ovn: {table} row: no uuid")))?;
                if !ovsdb::row_matches(one, &row) {
                    self.run(vec![ovsdb::update(table, vec![eq("_uuid", uuid(&u))], row)])
                        .await?;
                }
                Ok(u)
            }
            many => Err(ApiError::conflict(format!(
                "ovn: {} {table} rows match {}",
                many.len(),
                Value::Array(cond)
            ))),
        }
    }
}

fn switch_name(net: &Id) -> String {
    format!("ps-{net}")
}

fn provnet_name(net: &Id) -> String {
    format!("provnet-{net}")
}

fn by_name(name: &str) -> Vec<Value> {
    vec![eq("name", json!(name))]
}

fn tagged(key: &str, id: &Id) -> Vec<Value> {
    vec![includes("external_ids", map([(key, id.to_string())]))]
}

/// Operations that put the row inserted as `new` on switch `switch`, and
/// abort if that switch does not exist (an unreferenced switch port would
/// otherwise be dropped silently at commit).
fn onto_switch(switch: &str) -> Vec<Value> {
    vec![
        ovsdb::present("Logical_Switch", by_name(switch)),
        ovsdb::mutate(
            "Logical_Switch",
            by_name(switch),
            vec![json!(["ports", "insert", set([ovsdb::named("new")])])],
        ),
    ]
}

/// A stable locally administered MAC for a subnet's DHCP server, from the
/// random tail of its id.
fn server_mac(id: &Id) -> String {
    let b = id.uuid.as_bytes();
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[11], b[12], b[13], b[14], b[15]
    )
}

/// `{a, b}`, OVN's spelling of a list inside an option value.
fn braces(items: impl IntoIterator<Item = String>) -> String {
    format!("{{{}}}", items.into_iter().collect::<Vec<_>>().join(", "))
}

fn dhcp_options(net: &Network, subnet: &Subnet) -> Vec<(String, String)> {
    let s = &subnet.spec;
    let mac = server_mac(&subnet.meta.id);
    let mut o: Vec<(String, String)> = Vec::new();
    let dns = (!s.dns_nameservers.is_empty())
        .then(|| braces(s.dns_nameservers.iter().map(IpAddr::to_string)));
    if s.cidr.addr().is_ipv4() {
        let server = s.gateway.or_else(|| s.cidr.hosts().next());
        if let Some(server) = server {
            o.push(("server_id".into(), server.to_string()));
        }
        o.push(("server_mac".into(), mac));
        o.push(("lease_time".into(), "43200".into()));
        if let Some(gw) = s.gateway {
            o.push(("router".into(), gw.to_string()));
        }
        if let Some(dns) = dns {
            o.push(("dns_server".into(), dns));
        }
        o.push(("mtu".into(), net.spec.mtu.to_string()));
        let mut routes: Vec<String> = s
            .host_routes
            .iter()
            .map(|r| format!("{},{}", r.destination, r.nexthop))
            .collect();
        if !routes.is_empty() {
            if let Some(gw) = s.gateway {
                routes.push(format!("0.0.0.0/0,{gw}"));
            }
            o.push(("classless_static_route".into(), braces(routes)));
        }
    } else {
        o.push(("server_id".into(), mac));
        if let Some(dns) = dns {
            o.push(("dns_server".into(), dns));
        }
    }
    o
}

/// The DHCP options row on `net` whose range holds one of `ips`, per family.
fn pick_dhcp(rows: &[Value], ips: &[IpAddr], v4: bool) -> Vec<Value> {
    rows.iter()
        .filter_map(|r| {
            let cidr: ipnet::IpNet = r.get("cidr")?.as_str()?.parse().ok()?;
            (cidr.addr().is_ipv4() == v4 && ips.iter().any(|ip| cidr.contains(ip)))
                .then(|| ovsdb::row_uuid(r))
                .flatten()
        })
        .take(1)
        .map(|u| uuid(&u))
        .collect()
}

#[async_trait]
impl NetBackend for OvnBackend {
    fn name(&self) -> &'static str {
        "ovn"
    }

    fn attach(&self, leadership: &Leadership, store: &Store) {
        let _ = self.attached.set(Attached {
            leadership: leadership.clone(),
            store: store.clone(),
        });
    }

    /// `Logical_Switch`:
    /// * `name = ps-<net id>`
    /// * `other_config:mtu = <spec.mtu>`
    /// * Overlay: nothing else; OVN picks the Geneve tunnel key.
    /// * Provider: a `Logical_Switch_Port` named `provnet-<net id>` with
    ///   `type=localnet`, `options:network_name=<physnet>`,
    ///   `addresses=["unknown"]`, and `tag=<vlan>` when set.
    /// * `external` networks additionally get
    ///   `external_ids:pistack:external=true` so routers can find them.
    /// * `other_config:vlan-passthru` stays unset (no Q-in-Q).
    async fn ensure_network(&self, net: &Network) -> ApiResult<()> {
        let id = &net.meta.id;
        let name = switch_name(id);
        let mut ext = self.tags(id, net.meta.project.as_ref());
        if net.spec.external {
            ext.push(("pistack:external".into(), "true".into()));
        }
        let row = json!({
            "name": name,
            "other_config": map([("mtu", net.spec.mtu.to_string())]),
            "external_ids": map(ext.clone()),
        });
        self.ensure_row("Logical_Switch", by_name(&name), row, vec![])
            .await?;
        if let NetworkKind::Provider { vlan, physnet } = &net.spec.kind {
            let uplink = provnet_name(id);
            let tag = vlan.map(|v| set([json!(v)])).unwrap_or_else(|| set([]));
            let row = json!({
                "name": uplink,
                "type": "localnet",
                "addresses": "unknown",
                "options": map([("network_name", physnet.as_str())]),
                "tag": tag,
                "external_ids": map(ext),
            });
            self.ensure_row(
                "Logical_Switch_Port",
                by_name(&uplink),
                row,
                onto_switch(&name),
            )
            .await?;
        }
        Ok(())
    }

    /// Delete the `Logical_Switch`; OVN drops its ports with it. Also
    /// removes the network's `DHCP_Options` rows. A second delete succeeds.
    async fn delete_network(&self, id: &Id) -> ApiResult<()> {
        self.run(vec![
            ovsdb::delete("Logical_Switch", by_name(&switch_name(id))),
            ovsdb::delete("DHCP_Options", tagged("pistack:network", id)),
        ])
        .await?;
        Ok(())
    }

    /// `DHCP_Options` (one row per subnet with `enable_dhcp`):
    /// * `cidr = <spec.cidr>`
    /// * `options:server_id = <gateway or first host address>`,
    ///   `options:server_mac = <a stable locally administered mac>`,
    ///   `options:lease_time = 43200`, `options:router = <gateway>`,
    ///   `options:dns_server = {<dns_nameservers>}`,
    ///   `options:mtu = <network mtu>`,
    ///   `options:classless_static_route = {<host_routes>, 0.0.0.0/0,<gw>}`
    ///   when there are host routes.
    /// * IPv6: `options:server_id` is the server mac, `options:dns_server`
    ///   as above; no router option.
    /// * `external_ids:pistack:subnet = <subnet id>`,
    ///   `external_ids:pistack:network = <net id>`.
    ///
    /// A subnet without DHCP has no row; turning DHCP off deletes it.
    /// Fabric allocates every address itself, so the switch does not get
    /// `other_config:subnet`.
    async fn ensure_subnet(&self, net: &Network, subnet: &Subnet) -> ApiResult<()> {
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

    /// Delete the subnet's `DHCP_Options` row. Ports referring to it lose
    /// the reference with it.
    async fn delete_subnet(&self, id: &Id) -> ApiResult<()> {
        self.run(vec![ovsdb::delete(
            "DHCP_Options",
            tagged("pistack:subnet", id),
        )])
        .await?;
        Ok(())
    }

    /// `Logical_Switch_Port` on `ps-<net id>`:
    /// * `name = <port id>`, `type = ""` (VIF) for machine ports. Router
    ///   ports are written with their router (see [`Self::ensure_router`]);
    ///   here they only get a binding.
    /// * `addresses = ["<mac> <ip1> <ip2>..."]`, or `["unknown"]` before a
    ///   mac is assigned.
    /// * `port_security = ["<mac> <ip1>..."]` when `spec.port_security`,
    ///   else `[]`.
    /// * `dhcpv4_options` / `dhcpv6_options` = the network's `DHCP_Options`
    ///   row whose range holds one of the port's addresses.
    /// * `external_ids:pistack:id`, `pistack:device = <device id>`,
    ///   `pistack:project`, `pistack:term`.
    /// * Security groups: not yet. A later change adds the port to each
    ///   group's `Port_Group.ports`; when `port_security` is off the port
    ///   joins no group and receives no ACLs (OVN default allow).
    ///
    /// Binding: the agent sets `external_ids:iface-id = <port id>` on the
    /// OVS interface on the host. The returned `PortBinding` is
    /// `{vif_type: "ovs", vif_details: {bridge: "br-int", iface_id: <port id>}}`.
    async fn ensure_port(
        &self,
        net: &Network,
        port: &Port,
        _groups: &[SecurityGroup],
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
            ("unknown".to_string(), Vec::new())
        } else {
            let mut a = mac.to_string();
            for ip in ips {
                a.push(' ');
                a.push_str(&ip.to_string());
            }
            let sec = if port.spec.port_security {
                vec![json!(a.clone())]
            } else {
                Vec::new()
            };
            (a, sec)
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
        let name = id.to_string();
        let row = json!({
            "name": name,
            "addresses": addresses,
            "port_security": set(security),
            "dhcpv4_options": set(pick_dhcp(&dhcp, ips, true)),
            "dhcpv6_options": set(pick_dhcp(&dhcp, ips, false)),
            "external_ids": map(ext),
        });
        let switch = switch_name(&net.meta.id);
        self.ensure_row(
            "Logical_Switch_Port",
            by_name(&name),
            row,
            onto_switch(&switch),
        )
        .await?;
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

    /// Take the port off its switch and delete it. A second delete
    /// succeeds. (Later: also remove it from every `Port_Group.ports`.)
    async fn delete_port(&self, id: &Id) -> ApiResult<()> {
        let name = id.to_string();
        let found = self
            .run(vec![ovsdb::select("Logical_Switch_Port", by_name(&name))])
            .await?;
        for row in ovsdb::rows(&found[0]) {
            let Some(u) = ovsdb::row_uuid(&row) else {
                continue;
            };
            self.run(vec![
                ovsdb::mutate(
                    "Logical_Switch",
                    vec![includes("ports", uuid(&u))],
                    vec![json!(["ports", "delete", set([uuid(&u)])])],
                ),
                ovsdb::delete("Logical_Switch_Port", vec![eq("_uuid", uuid(&u))]),
            ])
            .await?;
        }
        Ok(())
    }

    /// `Logical_Router`:
    /// * `name = pr-<rtr id>`, `options:always_learn_from_arp_request=false`,
    ///   `options:dynamic_neigh_routers=true`.
    /// * Per interface subnet: `Logical_Router_Port`
    ///   `name = lrp-<port id>`, `mac = <port mac>`,
    ///   `networks = ["<gateway>/<prefix>"]`, linked from the
    ///   `Logical_Switch_Port` `type=router`,
    ///   `options:router-port=lrp-<port id>`, `addresses=["router"]`.
    /// * Gateway: an LRP on the external switch the same way, plus
    ///   `Gateway_Chassis` rows (one per gateway-capable chassis, priority
    ///   descending) so the port is scheduled; a default
    ///   `Logical_Router_Static_Route` `ip_prefix=0.0.0.0/0`,
    ///   `nexthop=<external subnet gateway>`; and a `NAT` row
    ///   `type=snat`, `logical_ip=<each interface cidr>`,
    ///   `external_ip=<gateway port fixed ip>` so tenant traffic can leave.
    /// * `spec.routes` become `Logical_Router_Static_Route` rows
    ///   (`ip_prefix`, `nexthop`).
    /// * Everything carries `external_ids:pistack:id = <rtr id>`; the
    ///   reconciler diffs existing LRP/NAT/route rows against the view and
    ///   removes the extras.
    async fn ensure_router(&self, _view: &RouterView) -> ApiResult<()> {
        Err(self.todo("ensure_router"))
    }

    /// `lr-del pr-<rtr id>` (cascades LRPs, NAT, routes); delete the
    /// matching `type=router` LSPs on each switch.
    async fn delete_router(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_router"))
    }

    /// One `Port_Group` per security group:
    /// * `name = pg_<sg uuid>`, `ports = {member LSPs}`,
    ///   `external_ids:pistack:id = <sg id>`.
    /// * OVN derives `Address_Set`s `pg_<uuid>_ip4` / `_ip6` from the
    ///   group's ports automatically; those are what `Remote::Group` rules
    ///   match on.
    /// * Per rule an `ACL` in `Port_Group.acls`:
    ///   `direction = to-lport` (ingress) or `from-lport` (egress),
    ///   `priority = 1002`, `action = allow-related`,
    ///   `match = "outport == @pg_<uuid> && ip4 && ip4.src == <cidr> && tcp && tcp.dst >= min && tcp.dst <= max"`
    ///   (ingress; `inport`/`ip4.dst` for egress; `ip6` for IPv6;
    ///   `Remote::Group(g)` → `ip4.src == $pg_<g uuid>_ip4`;
    ///   `Remote::Any` omits the address term; `protocol=None|Any`
    ///   omits the L4 term; `icmp` → `icmp4`/`icmp6`).
    /// * Two implicit drop ACLs per group at `priority = 1001`:
    ///   `to-lport outport == @pg && ip` and `from-lport inport == @pg && ip`,
    ///   `action = drop`, so a port with groups is default-deny both ways
    ///   except for what its rules allow.
    /// * A cloud-wide `Port_Group` `pg_drop_all` (priority 1000 drop) and
    ///   allow ACLs for DHCP (`udp.src == 68 && udp.dst == 67`) and
    ///   IPv6 ND are installed once by the reconciler on first use.
    /// * ACL `external_ids:pistack:rule = <rule uuid>` lets the reconciler
    ///   diff rules without recreating the whole group.
    async fn apply_security_group(&self, _sg: &SecurityGroup, _members: &[Port]) -> ApiResult<()> {
        Err(self.todo("apply_security_group"))
    }

    /// `pg-del pg_<sg uuid>`; OVN cascades its ACLs.
    async fn delete_security_group(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_security_group"))
    }

    /// `NAT` row on the router that has an interface on the target port's
    /// subnet (`view.router`):
    /// * `type = dnat_and_snat`, `external_ip = <fip address>`,
    ///   `logical_ip = <target fixed ip>`, `logical_port = <port id>`,
    ///   `external_mac = <gateway port mac>` (distributed FIP, so the
    ///   chassis hosting the VM answers ARP directly),
    ///   `options:stateless = false`,
    ///   `external_ids:pistack:id = <fip id>`.
    /// * Disassociate (`target: None`): remove the NAT row matching
    ///   `external_ids:pistack:id`.
    /// * The address itself is reserved in the external subnet by fabric's
    ///   IPAM; OVN needs nothing for an unassociated FIP.
    async fn associate_fip(&self, _view: &FipView) -> ApiResult<()> {
        Err(self.todo("associate_fip"))
    }

    /// Remove any `NAT` row with `external_ids:pistack:id = <fip id>`.
    async fn delete_fip(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_fip"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ovsdb::fake::FakeDb;
    use crate::types::{
        HostRoute, NetworkSpec, NetworkStatus, PortSpec, PortStatus, SubnetSpec, SubnetStatus,
    };
    use pistack_types::{kind, Meta};

    fn backend() -> (Arc<FakeDb>, OvnBackend) {
        let db = Arc::new(FakeDb::new());
        let b = OvnBackend::with_transport("fake", db.clone());
        (db, b)
    }

    fn net(kind: NetworkKind) -> Network {
        Network {
            meta: Meta::new(kind::NETWORK, "n", Some(Id::new(kind::PROJECT))),
            spec: NetworkSpec {
                kind,
                ..Default::default()
            },
            status: NetworkStatus::default(),
        }
    }

    fn subnet(net: &Network, cidr: &str, gw: &str) -> Subnet {
        Subnet {
            meta: Meta::new(kind::SUBNET, "s", net.meta.project.clone()),
            spec: SubnetSpec {
                network: net.meta.id.clone(),
                cidr: cidr.parse().unwrap(),
                gateway: Some(gw.parse().unwrap()),
                allocation_pools: Vec::new(),
                dns_nameservers: vec!["10.0.0.53".parse().unwrap()],
                enable_dhcp: true,
                host_routes: Vec::new(),
            },
            status: SubnetStatus::default(),
        }
    }

    fn port(net: &Network, mac: &str, ips: &[&str]) -> Port {
        Port {
            meta: Meta::new(kind::PORT, "p", net.meta.project.clone()),
            spec: PortSpec {
                network: net.meta.id.clone(),
                fixed_ips: ips.iter().map(|i| i.parse().unwrap()).collect(),
                mac: mac.into(),
                device: Some(Id::new(kind::MACHINE)),
                ..Default::default()
            },
            status: PortStatus::default(),
        }
    }

    fn ext(row: &Value, key: &str) -> Option<String> {
        let ps = row["external_ids"][1].as_array()?;
        ps.iter()
            .find(|p| p[0] == key)
            .and_then(|p| p[1].as_str().map(str::to_string))
    }

    fn opt(row: &Value, col: &str, key: &str) -> Option<String> {
        let ps = row[col][1].as_array()?;
        ps.iter()
            .find(|p| p[0] == key)
            .and_then(|p| p[1].as_str().map(str::to_string))
    }

    #[tokio::test]
    async fn a_network_becomes_a_switch_and_a_repeat_writes_nothing() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        b.ensure_network(&n).await.unwrap();
        let ls = db.rows("Logical_Switch");
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0]["name"], json!(format!("ps-{}", n.meta.id)));
        assert_eq!(opt(&ls[0], "other_config", "mtu").as_deref(), Some("1500"));
        assert_eq!(ext(&ls[0], "pistack:id"), Some(n.meta.id.to_string()));
        assert_eq!(
            ext(&ls[0], "pistack:project"),
            n.meta.project.as_ref().map(Id::to_string)
        );
        assert_eq!(ext(&ls[0], "pistack:term").as_deref(), Some("0"));

        let writes = db.writes();
        b.ensure_network(&n).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");
        assert_eq!(db.rows("Logical_Switch").len(), 1);

        let mut bigger = n.clone();
        bigger.spec.mtu = 9000;
        b.ensure_network(&bigger).await.unwrap();
        let ls = db.rows("Logical_Switch");
        assert_eq!(ls.len(), 1);
        assert_eq!(opt(&ls[0], "other_config", "mtu").as_deref(), Some("9000"));
    }

    #[tokio::test]
    async fn a_provider_network_gets_a_localnet_uplink_on_its_switch() {
        let (db, b) = backend();
        let n = net(NetworkKind::Provider {
            vlan: Some(42),
            physnet: "physnet1".into(),
        });
        b.ensure_network(&n).await.unwrap();
        b.ensure_network(&n).await.unwrap();
        let lsp = db.rows("Logical_Switch_Port");
        assert_eq!(lsp.len(), 1);
        assert_eq!(lsp[0]["name"], json!(format!("provnet-{}", n.meta.id)));
        assert_eq!(lsp[0]["type"], "localnet");
        assert_eq!(
            opt(&lsp[0], "options", "network_name").as_deref(),
            Some("physnet1")
        );
        assert_eq!(ovsdb::canon(&lsp[0]["tag"]), json!(42));
        let ls = db.rows("Logical_Switch");
        assert_eq!(
            ovsdb::elems(ls[0].get("ports")),
            vec![lsp[0]["_uuid"].clone()]
        );
    }

    #[tokio::test]
    async fn a_dhcp_subnet_gets_options_and_turning_dhcp_off_removes_them() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let mut s = subnet(&n, "10.20.0.0/24", "10.20.0.1");
        s.spec.host_routes = vec![HostRoute {
            destination: "192.168.5.0/24".parse().unwrap(),
            nexthop: "10.20.0.254".parse().unwrap(),
        }];
        b.ensure_network(&n).await.unwrap();
        b.ensure_subnet(&n, &s).await.unwrap();
        let d = db.rows("DHCP_Options");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0]["cidr"], "10.20.0.0/24");
        assert_eq!(
            opt(&d[0], "options", "server_id").as_deref(),
            Some("10.20.0.1")
        );
        assert_eq!(
            opt(&d[0], "options", "router").as_deref(),
            Some("10.20.0.1")
        );
        assert_eq!(
            opt(&d[0], "options", "lease_time").as_deref(),
            Some("43200")
        );
        assert_eq!(
            opt(&d[0], "options", "dns_server").as_deref(),
            Some("{10.0.0.53}")
        );
        assert_eq!(
            opt(&d[0], "options", "classless_static_route").as_deref(),
            Some("{192.168.5.0/24,10.20.0.254, 0.0.0.0/0,10.20.0.1}")
        );
        assert!(opt(&d[0], "options", "server_mac")
            .unwrap()
            .starts_with("02:"));
        assert_eq!(ext(&d[0], "pistack:network"), Some(n.meta.id.to_string()));

        let writes = db.writes();
        b.ensure_subnet(&n, &s).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        let mut off = s.clone();
        off.spec.enable_dhcp = false;
        b.ensure_subnet(&n, &off).await.unwrap();
        assert!(db.rows("DHCP_Options").is_empty());

        b.ensure_subnet(&n, &s).await.unwrap();
        b.delete_subnet(&s.meta.id).await.unwrap();
        b.delete_subnet(&s.meta.id).await.unwrap();
        assert!(db.rows("DHCP_Options").is_empty());
    }

    #[tokio::test]
    async fn a_port_joins_its_switch_with_addresses_security_and_dhcp() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let s4 = subnet(&n, "10.20.0.0/24", "10.20.0.1");
        let s6 = subnet(&n, "fd00:20::/64", "fd00:20::1");
        b.ensure_network(&n).await.unwrap();
        b.ensure_subnet(&n, &s4).await.unwrap();
        b.ensure_subnet(&n, &s6).await.unwrap();
        let p = port(&n, "fa:16:3e:00:00:01", &["10.20.0.5", "fd00:20::5"]);
        let binding = b.ensure_port(&n, &p, &[]).await.unwrap();
        assert_eq!(binding.vif_type, "ovs");
        assert_eq!(binding.vif_details["bridge"], "br-int");
        assert_eq!(binding.vif_details["iface_id"], p.meta.id.to_string());

        let lsp = db.rows("Logical_Switch_Port");
        assert_eq!(lsp.len(), 1);
        let want = "fa:16:3e:00:00:01 10.20.0.5 fd00:20::5";
        assert_eq!(lsp[0]["name"], json!(p.meta.id.to_string()));
        assert_eq!(ovsdb::canon(&lsp[0]["addresses"]), json!(want));
        assert_eq!(ovsdb::canon(&lsp[0]["port_security"]), json!(want));
        assert_eq!(
            ext(&lsp[0], "pistack:device"),
            p.spec.device.as_ref().map(Id::to_string)
        );
        let dhcp = db.rows("DHCP_Options");
        let uuid_of = |cidr: &str| {
            dhcp.iter()
                .find(|r| r["cidr"] == cidr)
                .map(|r| r["_uuid"].clone())
                .unwrap()
        };
        assert_eq!(
            ovsdb::canon(&lsp[0]["dhcpv4_options"]),
            uuid_of("10.20.0.0/24")
        );
        assert_eq!(
            ovsdb::canon(&lsp[0]["dhcpv6_options"]),
            uuid_of("fd00:20::/64")
        );
        let ls = db.rows("Logical_Switch");
        assert_eq!(
            ovsdb::elems(ls[0].get("ports")),
            vec![lsp[0]["_uuid"].clone()]
        );

        let writes = db.writes();
        b.ensure_port(&n, &p, &[]).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        let mut open = p.clone();
        open.spec.port_security = false;
        b.ensure_port(&n, &open, &[]).await.unwrap();
        let lsp = db.rows("Logical_Switch_Port");
        assert_eq!(ovsdb::canon(&lsp[0]["port_security"]), json!(["set", []]));

        b.delete_port(&p.meta.id).await.unwrap();
        b.delete_port(&p.meta.id).await.unwrap();
        assert!(db.rows("Logical_Switch_Port").is_empty());
        let ls = db.rows("Logical_Switch");
        assert!(ovsdb::elems(ls[0].get("ports")).is_empty());
    }

    #[tokio::test]
    async fn a_port_on_a_missing_switch_fails_instead_of_vanishing() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let p = port(&n, "fa:16:3e:00:00:02", &["10.20.0.6"]);
        assert!(b.ensure_port(&n, &p, &[]).await.is_err());
        assert!(db.rows("Logical_Switch_Port").is_empty());
    }

    #[tokio::test]
    async fn deleting_a_network_takes_its_ports_and_dhcp_with_it() {
        let (db, b) = backend();
        let n = net(NetworkKind::Overlay);
        let s = subnet(&n, "10.20.0.0/24", "10.20.0.1");
        b.ensure_network(&n).await.unwrap();
        b.ensure_subnet(&n, &s).await.unwrap();
        b.ensure_port(&n, &port(&n, "fa:16:3e:00:00:03", &["10.20.0.7"]), &[])
            .await
            .unwrap();
        b.delete_network(&n.meta.id).await.unwrap();
        b.delete_network(&n.meta.id).await.unwrap();
        assert!(db.rows("Logical_Switch").is_empty());
        assert!(db.rows("Logical_Switch_Port").is_empty());
        assert!(db.rows("DHCP_Options").is_empty());
    }

    #[tokio::test]
    async fn a_follower_never_writes_and_rows_carry_the_leaders_term() {
        let (db, b) = backend();
        let store = Store::in_memory().unwrap();
        store.put_meta(TERM_KEY, &7u64).unwrap();
        let leadership = Leadership::new(false);
        b.attach(&leadership, &store);
        let n = net(NetworkKind::Overlay);
        let err = b.ensure_network(&n).await.unwrap_err();
        assert!(err.message.contains("not leader"), "{}", err.message);
        assert_eq!(db.writes(), 0);
        assert!(db.rows("Logical_Switch").is_empty());

        leadership.set(true);
        b.ensure_network(&n).await.unwrap();
        let ls = db.rows("Logical_Switch");
        assert_eq!(ext(&ls[0], "pistack:term").as_deref(), Some("7"));
    }

    #[tokio::test]
    async fn the_routed_parts_still_say_not_implemented() {
        let (_db, b) = backend();
        let err = b.delete_router(&Id::new(kind::ROUTER)).await.unwrap_err();
        assert!(err.message.contains("not implemented"));
    }
}
