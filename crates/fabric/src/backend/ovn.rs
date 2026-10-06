// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! OVN northbound backend. Networks, subnets, machine ports, security
//! groups, routers and floating IPs are written to OVN's northbound
//! database. The doc comment on each method is its contract.
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
//!   A security group is `Port_Group.name = pg_<sg uuid without dashes>`
//!   (OVN names must be identifiers), and every port with port security
//!   also joins `pg_pistack_drop`. A router is `Logical_Router.name =
//!   pr-<rtr id>` and each of its ports is `lrp-<port id>`.
//! * Router gateway ports are scheduled on the chassis listed in
//!   `PISTACK_OVN_GATEWAY_CHASSIS`. With none listed the gateway port has
//!   no chassis, so outbound NAT and floating IPs do not leave the
//!   cluster.
//!
//! On each host, OVS and `ovn-controller` realise the switches. The agent
//! plugs a machine's tap or a pod's link into the integration bridge with
//! `external_ids:iface-id=<port id>`; the port binding below tells it which
//! bridge and id to use.

use super::ovsdb::{self, eq, includes, map, set, uuid, Transport};
use super::{FipView, NetBackend, RouterView};
use crate::types::{
    Direction, EtherType, Network, NetworkKind, Port, PortBinding, Protocol, Remote, Rule,
    SecurityGroup, Subnet,
};
use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id, Leadership};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

const DB: &str = "OVN_Northbound";

/// Environment variable naming the northbound database endpoint.
pub const NB_ENV: &str = "PISTACK_OVN_NB";
/// Where a host running the northbound database listens by default.
pub const DEFAULT_NB: &str = "unix:/var/run/ovn/ovnnb_db.sock";
/// The OVS bridge `ovn-controller` manages on every host.
pub const INTEGRATION_BRIDGE: &str = "br-int";
/// Environment variable listing the chassis that may host router gateway
/// ports, highest priority first, comma separated.
pub const GATEWAY_CHASSIS_ENV: &str = "PISTACK_OVN_GATEWAY_CHASSIS";
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
    /// Chassis for router gateway ports, highest priority first.
    gateway_chassis: Vec<String>,
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
        Self::with_transport(nb_endpoint, nb).with_gateway_chassis(chassis_from_env())
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
            gateway_chassis: Vec::new(),
        }
    }

    /// Chassis that may host router gateway ports, highest priority first.
    pub fn with_gateway_chassis(mut self, chassis: Vec<String>) -> Self {
        self.gateway_chassis = chassis;
        self
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

    /// Insert ops for `acls` tagged with `group`; returns their named refs.
    fn acl_inserts(&self, group: &str, acls: &[Acl], ops: &mut Vec<Value>) -> Vec<Value> {
        let term = self.term().to_string();
        acls.iter()
            .enumerate()
            .map(|(i, a)| {
                let name = format!("acl{i}");
                let mut ext = vec![
                    ("pistack:group".to_string(), group.to_string()),
                    ("pistack:term".to_string(), term.clone()),
                ];
                if let Some(r) = &a.rule {
                    ext.push(("pistack:rule".into(), r.clone()));
                }
                let row = json!({
                    "direction": a.direction,
                    "priority": a.priority,
                    "match": a.matches,
                    "action": a.action,
                    "external_ids": map(ext),
                });
                ops.push(ovsdb::insert("ACL", row, &name));
                ovsdb::named(&name)
            })
            .collect()
    }

    /// Make `Port_Group` `name` exist with `ext`. `acls` and `ports`, when
    /// given, replace what the group has if it differs; `None` leaves them
    /// alone (empty on a new group). Replaced ACLs are dropped by the
    /// database once nothing refers to them.
    async fn ensure_group(
        &self,
        name: &str,
        ext: Vec<(String, String)>,
        acls: Option<&[Acl]>,
        ports: Option<Vec<String>>,
    ) -> ApiResult<()> {
        let found = self
            .run(vec![
                ovsdb::select("Port_Group", by_name(name)),
                ovsdb::select(
                    "ACL",
                    vec![includes("external_ids", map([("pistack:group", name)]))],
                ),
            ])
            .await?;
        let groups = ovsdb::rows(&found[0]);
        let have_acls = ovsdb::rows(&found[1]);
        let mut ops = Vec::new();
        match groups.as_slice() {
            [] => {
                ops.push(ovsdb::absent("Port_Group", by_name(name)));
                let refs = acls
                    .map(|a| self.acl_inserts(name, a, &mut ops))
                    .unwrap_or_default();
                let ports = ports.unwrap_or_default();
                ops.push(ovsdb::insert(
                    "Port_Group",
                    json!({
                        "name": name,
                        "acls": set(refs),
                        "ports": set(ports.iter().map(|u| uuid(u))),
                        "external_ids": map(ext),
                    }),
                    "pg",
                ));
                self.run(ops).await?;
                Ok(())
            }
            [one] => {
                let u = ovsdb::row_uuid(one)
                    .ok_or_else(|| ApiError::unavailable("ovn: Port_Group row: no uuid"))?;
                let mut row = serde_json::Map::new();
                if let Some(want) = acls {
                    if !same_acls(&have_acls, want) {
                        let refs = self.acl_inserts(name, want, &mut ops);
                        row.insert("acls".into(), set(refs));
                    }
                }
                if let Some(mut want) = ports {
                    want.sort();
                    want.dedup();
                    if uuid_strings(one.get("ports")) != want {
                        row.insert("ports".into(), set(want.iter().map(|u| uuid(u))));
                    }
                }
                let ext = map(ext);
                if !ovsdb::row_matches(one, &json!({ "external_ids": ext })) {
                    row.insert("external_ids".into(), ext);
                }
                if !row.is_empty() {
                    ops.push(ovsdb::update(
                        "Port_Group",
                        vec![eq("_uuid", uuid(&u))],
                        Value::Object(row),
                    ));
                    self.run(ops).await?;
                }
                Ok(())
            }
            many => Err(ApiError::conflict(format!(
                "ovn: {} Port_Group rows named {name}",
                many.len()
            ))),
        }
    }

    /// The group for `sg` with its rules as ACLs. Groups its rules refer to
    /// are created empty first, so the address sets OVN derives from them
    /// exist before an ACL names them.
    async fn ensure_sg_group(
        &self,
        sg: &SecurityGroup,
        ports: Option<Vec<String>>,
    ) -> ApiResult<()> {
        for rule in &sg.spec.rules {
            if let Remote::Group(g) = &rule.remote {
                if g != &sg.meta.id {
                    self.ensure_group(&group_name(g), self.tags(g, None), None, None)
                        .await?;
                }
            }
        }
        let acls = group_acls(sg);
        self.ensure_group(
            &group_name(&sg.meta.id),
            self.tags(&sg.meta.id, sg.meta.project.as_ref()),
            Some(&acls),
            ports,
        )
        .await
    }

    async fn ensure_drop_group(&self) -> ApiResult<()> {
        let ext = vec![
            ("pistack:id".to_string(), DROP_GROUP.to_string()),
            ("pistack:term".to_string(), self.term().to_string()),
        ];
        self.ensure_group(DROP_GROUP, ext, Some(&drop_acls()), None)
            .await
    }

    /// Put switch port `lsp` in exactly the PiStack groups named `want`.
    /// Groups without `pistack:id` are never touched.
    async fn set_membership(&self, lsp: &str, want: &[String]) -> ApiResult<()> {
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

    /// Take every switch port matching `cond` off its switch and delete it.
    async fn drop_switch_ports(&self, cond: Vec<Value>) -> ApiResult<()> {
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

    /// One router port and its `type=router` peer on the switch.
    async fn ensure_router_port(&self, rtr: &Id, router: &str, want: &RouterPort) -> ApiResult<()> {
        let port = &want.port;
        let mac = port.spec.mac.trim();
        if mac.is_empty() {
            return Err(ApiError::unavailable(format!(
                "ovn: router port {} has no mac yet",
                port.meta.id
            )));
        }
        let lrp = lrp_name(&port.meta.id);
        let mut ext = self.tags(rtr, port.meta.project.as_ref());
        ext.push(("pistack:port".into(), port.meta.id.to_string()));
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
            self.set_gateway_chassis(rtr, &lrp, &lrp_uuid).await?;
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

    /// Point the gateway port at one `Gateway_Chassis` row per configured
    /// chassis. Rewrites the rows only when the chassis or priorities differ.
    async fn set_gateway_chassis(&self, rtr: &Id, lrp: &str, lrp_uuid: &str) -> ApiResult<()> {
        let n = self.gateway_chassis.len();
        let mut want: Vec<(String, i64)> = self
            .gateway_chassis
            .iter()
            .enumerate()
            .map(|(i, c)| (c.clone(), (n - i) as i64))
            .collect();
        let at = vec![eq("_uuid", uuid(lrp_uuid))];
        let found = self
            .run(vec![ovsdb::select("Logical_Router_Port", at.clone())])
            .await?;
        let held = uuid_strings(
            ovsdb::rows(&found[0])
                .first()
                .and_then(|r| r.get("gateway_chassis")),
        );
        let old = if held.is_empty() {
            Vec::new()
        } else {
            let ops = held
                .iter()
                .map(|u| ovsdb::select("Gateway_Chassis", vec![eq("_uuid", uuid(u))]))
                .collect();
            let out = self.run(ops).await?;
            out.iter().flat_map(ovsdb::rows).collect()
        };
        let mut have: Vec<(String, i64)> = old
            .iter()
            .map(|r| {
                (
                    r["chassis_name"].as_str().unwrap_or("").to_string(),
                    r["priority"].as_i64().unwrap_or(0),
                )
            })
            .collect();
        have.sort();
        let order = want.clone();
        want.sort();
        if have == want {
            return Ok(());
        }
        let mut ops = Vec::new();
        let mut names = Vec::new();
        for (i, (chassis, priority)) in order.iter().enumerate() {
            let n = format!("gc{i}");
            ops.push(ovsdb::insert(
                "Gateway_Chassis",
                json!({
                    "name": format!("{lrp}_{chassis}"),
                    "chassis_name": chassis,
                    "priority": priority,
                    "external_ids": map(self.tags(rtr, None)),
                }),
                &n,
            ));
            names.push(ovsdb::named(&n));
        }
        ops.push(ovsdb::update(
            "Logical_Router_Port",
            at,
            json!({ "gateway_chassis": set(names) }),
        ));
        for u in &held {
            ops.push(ovsdb::delete("Gateway_Chassis", vec![eq("_uuid", uuid(u))]));
        }
        self.run(ops).await?;
        Ok(())
    }

    /// Make the rows of `table` that router `rtr` owns (tagged with its id,
    /// held in router column `col`) the set `want`. Rows are the same when
    /// the `key` columns match; any difference replaces the whole set in one
    /// transaction, and a match writes nothing.
    async fn ensure_router_set(
        &self,
        rtr: &Id,
        router: &str,
        table: &str,
        col: &str,
        want: Vec<Value>,
        key: &[&str],
    ) -> ApiResult<()> {
        let found = self
            .run(vec![ovsdb::select(table, tagged("pistack:id", rtr))])
            .await?;
        let have = ovsdb::rows(&found[0]);
        let keys = |rows: &[Value]| {
            let mut out: Vec<String> = rows
                .iter()
                .map(|r| {
                    let k: Vec<Value> = key
                        .iter()
                        .map(|c| ovsdb::canon(r.get(*c).unwrap_or(&json!(["set", []]))))
                        .collect();
                    Value::Array(k).to_string()
                })
                .collect();
            out.sort();
            out
        };
        if keys(&have) == keys(&want) {
            return Ok(());
        }
        let mut ops = vec![ovsdb::present("Logical_Router", by_name(router))];
        let old: Vec<Value> = have
            .iter()
            .filter_map(ovsdb::row_uuid)
            .map(|u| uuid(&u))
            .collect();
        if !old.is_empty() {
            ops.push(ovsdb::mutate(
                "Logical_Router",
                by_name(router),
                vec![json!([col, "delete", set(old.clone())])],
            ));
            for u in old {
                ops.push(ovsdb::delete(table, vec![eq("_uuid", u)]));
            }
        }
        let mut names = Vec::new();
        for (i, row) in want.into_iter().enumerate() {
            let n = format!("row{i}");
            ops.push(ovsdb::insert(table, row, &n));
            names.push(ovsdb::named(&n));
        }
        if !names.is_empty() {
            ops.push(ovsdb::mutate(
                "Logical_Router",
                by_name(router),
                vec![json!([col, "insert", set(names)])],
            ));
        }
        self.run(ops).await?;
        Ok(())
    }
}

/// A router port as the router wants it.
struct RouterPort {
    port: Port,
    /// `<address>/<prefix>` on the port's subnet.
    network: String,
    gateway: bool,
}

fn chassis_from_env() -> Vec<String> {
    std::env::var(GATEWAY_CHASSIS_ENV)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .collect()
}

fn router_name(id: &Id) -> String {
    format!("pr-{id}")
}

fn lrp_name(port: &Id) -> String {
    format!("lrp-{port}")
}

/// `<address>/<prefix>` for a router port on `subnet`: the port's address
/// inside the subnet, else the subnet gateway.
fn port_network(port: &Port, subnet: &Subnet) -> ApiResult<String> {
    let cidr = subnet.spec.cidr;
    let ip = port
        .spec
        .fixed_ips
        .iter()
        .copied()
        .find(|ip| cidr.contains(ip))
        .or(subnet.spec.gateway)
        .ok_or_else(|| {
            ApiError::unavailable(format!(
                "ovn: router port {} has no address on subnet {}",
                port.meta.id, subnet.meta.id
            ))
        })?;
    Ok(format!("{ip}/{}", cidr.prefix_len()))
}

/// Operations that put the row inserted as `new` into router `router`'s
/// column `col`, and abort if that router does not exist.
fn onto_router(router: &str, col: &str) -> Vec<Value> {
    vec![
        ovsdb::present("Logical_Router", by_name(router)),
        ovsdb::mutate(
            "Logical_Router",
            by_name(router),
            vec![json!([col, "insert", set([ovsdb::named("new")])])],
        ),
    ]
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

/// Every port with port security joins this group. Its ACLs drop all IP
/// traffic both ways at a lower priority than group rules, and let DHCP
/// requests out.
const DROP_GROUP: &str = "pg_pistack_drop";
const DROP_PRIORITY: i64 = 1001;
const RULE_PRIORITY: i64 = 1002;

/// `pg_<sg uuid without dashes>`; OVN port group names must be identifiers.
fn group_name(sg: &Id) -> String {
    format!("pg_{}", sg.uuid.simple())
}

/// One ACL as fabric wants it. `rule` only feeds `external_ids`.
#[derive(Clone, Debug)]
struct Acl {
    direction: &'static str,
    priority: i64,
    matches: String,
    action: &'static str,
    rule: Option<String>,
}

impl Acl {
    fn key(&self) -> (String, i64, String, String) {
        (
            self.direction.to_string(),
            self.priority,
            self.matches.clone(),
            self.action.to_string(),
        )
    }
}

fn acl_row_key(row: &Value) -> (String, i64, String, String) {
    let s = |c: &str| row.get(c).and_then(Value::as_str).unwrap_or("").to_string();
    (
        s("direction"),
        row.get("priority").and_then(Value::as_i64).unwrap_or(-1),
        s("match"),
        s("action"),
    )
}

/// The ACL for one rule on group `group`, or `None` when the rule cannot
/// match anything (a remote address of the other family).
fn rule_acl(group: &str, rule: &Rule) -> Option<Acl> {
    let fam = match rule.ethertype {
        EtherType::IPv4 => "ip4",
        EtherType::IPv6 => "ip6",
    };
    let (direction, port_side, addr_side) = match rule.direction {
        Direction::Ingress => ("to-lport", "outport", "src"),
        Direction::Egress => ("from-lport", "inport", "dst"),
    };
    let mut m = format!("{port_side} == @{group} && {fam}");
    match &rule.remote {
        Remote::Any => {}
        Remote::Cidr(c) => {
            if c.addr().is_ipv4() != (fam == "ip4") {
                return None;
            }
            m.push_str(&format!(" && {fam}.{addr_side} == {}", c.trunc()));
        }
        Remote::Group(g) => {
            m.push_str(&format!(
                " && {fam}.{addr_side} == ${}_{fam}",
                group_name(g)
            ));
        }
    }
    match rule.protocol {
        Some(p @ (Protocol::Tcp | Protocol::Udp)) => {
            let l4 = if p == Protocol::Tcp { "tcp" } else { "udp" };
            m.push_str(&format!(" && {l4}"));
            if let Some(r) = rule.port_range {
                if r.min == r.max {
                    m.push_str(&format!(" && {l4}.dst == {}", r.min));
                } else {
                    m.push_str(&format!(
                        " && {l4}.dst >= {} && {l4}.dst <= {}",
                        r.min, r.max
                    ));
                }
            }
        }
        Some(Protocol::Icmp) => m.push_str(if fam == "ip4" {
            " && icmp4"
        } else {
            " && icmp6"
        }),
        Some(Protocol::Any) | None => {}
    }
    Some(Acl {
        direction,
        priority: RULE_PRIORITY,
        matches: m,
        action: "allow-related",
        rule: Some(rule.id.to_string()),
    })
}

fn group_acls(sg: &SecurityGroup) -> Vec<Acl> {
    let name = group_name(&sg.meta.id);
    let mut out: Vec<Acl> = Vec::new();
    for a in sg.spec.rules.iter().filter_map(|r| rule_acl(&name, r)) {
        if !out.iter().any(|o| o.key() == a.key()) {
            out.push(a);
        }
    }
    out
}

fn drop_acls() -> Vec<Acl> {
    let g = DROP_GROUP;
    let acl = |direction, priority, matches: String, action| Acl {
        direction,
        priority,
        matches,
        action,
        rule: None,
    };
    vec![
        acl(
            "from-lport",
            DROP_PRIORITY,
            format!("inport == @{g} && ip"),
            "drop",
        ),
        acl(
            "to-lport",
            DROP_PRIORITY,
            format!("outport == @{g} && ip"),
            "drop",
        ),
        acl(
            "from-lport",
            RULE_PRIORITY,
            format!("inport == @{g} && udp && udp.src == 68 && udp.dst == 67"),
            "allow",
        ),
        acl(
            "from-lport",
            RULE_PRIORITY,
            format!("inport == @{g} && udp && udp.src == 546 && udp.dst == 547"),
            "allow",
        ),
    ]
}

fn same_acls(have: &[Value], want: &[Acl]) -> bool {
    let mut h: Vec<_> = have.iter().map(acl_row_key).collect();
    let mut w: Vec<_> = want.iter().map(Acl::key).collect();
    h.sort();
    w.sort();
    h == w
}

fn uuid_strings(v: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = ovsdb::elems(v)
        .iter()
        .filter_map(|e| e.get(1).and_then(Value::as_str).map(str::to_string))
        .collect();
    out.sort();
    out
}

fn ext_value(row: &Value, key: &str) -> Option<String> {
    row["external_ids"][1]
        .as_array()?
        .iter()
        .find(|p| p[0] == key)
        .and_then(|p| p[1].as_str().map(str::to_string))
}

/// Operations that take each NAT row off whatever router holds it and
/// delete it.
fn unhook_nat(rows: &[Value]) -> Vec<Value> {
    let mut ops = Vec::new();
    for row in rows {
        let Some(u) = ovsdb::row_uuid(row) else {
            continue;
        };
        ops.push(ovsdb::mutate(
            "Logical_Router",
            vec![includes("nat", uuid(&u))],
            vec![json!(["nat", "delete", set([uuid(&u)])])],
        ));
        ops.push(ovsdb::delete("NAT", vec![eq("_uuid", uuid(&u))]));
    }
    ops
}

fn ext_has(row: &Value, key: &str) -> bool {
    row["external_ids"][1]
        .as_array()
        .is_some_and(|ps| ps.iter().any(|p| p[0] == key))
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
    /// * Security groups: with `port_security` the port joins
    ///   `pg_pistack_drop` and each of its groups' `Port_Group`, and leaves
    ///   any other PiStack group; without it the port is in no group and
    ///   gets no ACLs (OVN default allow).
    ///
    /// Binding: the agent sets `external_ids:iface-id = <port id>` on the
    /// OVS interface on the host. The returned `PortBinding` is
    /// `{vif_type: "ovs", vif_details: {bridge: "br-int", iface_id: <port id>}}`.
    async fn ensure_port(
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

    /// Take the port off its switch and delete it. A second delete
    /// succeeds. `Port_Group.ports` refers to switch ports weakly, so the
    /// port leaves its groups with it.
    async fn delete_port(&self, id: &Id) -> ApiResult<()> {
        self.drop_switch_ports(by_name(&id.to_string())).await
    }

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
    async fn ensure_router(&self, view: &RouterView) -> ApiResult<()> {
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
                    "ovn: router {id} gateway port has no subnet on network {}",
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

    /// Delete `pr-<rtr id>`; its LRPs, NAT rows, routes and gateway chassis
    /// go with it (none of those tables is a root table). Then take the
    /// router's `type=router` ports off their switches. A second delete
    /// succeeds.
    async fn delete_router(&self, id: &Id) -> ApiResult<()> {
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

    /// One `Port_Group` per security group:
    /// * `name = pg_<sg uuid without dashes>`, `ports = {member switch
    ///   ports with port security}`, `external_ids:pistack:id = <sg id>`.
    /// * OVN derives address sets `pg_<uuid>_ip4` / `_ip6` from the
    ///   group's ports; `Remote::Group` rules match on those. A group a
    ///   rule names is created empty if it does not exist yet.
    /// * Per rule an `ACL` in `Port_Group.acls`: `direction = to-lport`
    ///   (ingress) or `from-lport` (egress), `priority = 1002`,
    ///   `action = allow-related`, and a match such as
    ///   `outport == @pg_<uuid> && ip4 && ip4.src == 10.0.0.0/8 && tcp && tcp.dst == 22`
    ///   (`inport` and `ip4.dst` for egress, `ip6` for IPv6, `icmp4` /
    ///   `icmp6` for ICMP; no address term for `Remote::Any`, no L4 term
    ///   for any protocol). A rule whose remote address is of the other
    ///   family matches nothing and gets no ACL.
    /// * Default deny lives in one shared group, `pg_pistack_drop`, that
    ///   every port with port security joins: drop all IP both ways at
    ///   `priority = 1001`, allow DHCP and DHCPv6 requests out at 1002.
    /// * ACLs carry `external_ids:pistack:group = <group name>` and
    ///   `pistack:rule = <rule id>`. When the rules change, the group's
    ///   ACL set is replaced in one transaction; unchanged rules write
    ///   nothing.
    async fn apply_security_group(&self, sg: &SecurityGroup, members: &[Port]) -> ApiResult<()> {
        let names: Vec<String> = members
            .iter()
            .filter(|p| {
                p.spec.port_security && !p.spec.mac.trim().is_empty() && !p.is_router_port()
            })
            .map(|p| p.meta.id.to_string())
            .collect();
        let ports = if names.is_empty() {
            Vec::new()
        } else {
            let ops = names
                .iter()
                .map(|n| ovsdb::select("Logical_Switch_Port", by_name(n)))
                .collect();
            let out = self.run(ops).await?;
            out.iter()
                .flat_map(ovsdb::rows)
                .filter_map(|r| ovsdb::row_uuid(&r))
                .collect()
        };
        self.ensure_sg_group(sg, Some(ports)).await
    }

    /// Delete `pg_<sg uuid>`; the database drops its ACLs with it.
    async fn delete_security_group(&self, id: &Id) -> ApiResult<()> {
        self.run(vec![ovsdb::delete("Port_Group", by_name(&group_name(id)))])
            .await?;
        Ok(())
    }

    /// `NAT` row on the router that has an interface on the target port's
    /// subnet (`view.router`):
    /// * `type = dnat_and_snat`, `external_ip = <fip address>`,
    ///   `logical_ip = <target fixed ip>`, `logical_port = <port id>`,
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
    ///   is an `unavailable` error, retried by the reconciler.
    /// * The address itself is reserved in the external subnet by fabric's
    ///   IPAM; OVN needs nothing for an unassociated FIP.
    async fn associate_fip(&self, view: &FipView) -> ApiResult<()> {
        let fip = &view.fip;
        let id = &fip.meta.id;
        let Some((port, fixed)) = &view.target else {
            return self.delete_fip(id).await;
        };
        let address = fip.spec.address.ok_or_else(|| {
            ApiError::unavailable(format!("ovn: floating ip {id} has no address yet"))
        })?;
        let router = view.router.as_ref().ok_or_else(|| {
            ApiError::unavailable(format!(
                "ovn: no router joins port {} to network {}",
                port.meta.id, fip.spec.network
            ))
        })?;
        let mut ext = self.tags(id, fip.meta.project.as_ref());
        ext.push(("pistack:router".into(), router.meta.id.to_string()));
        let mac = port.spec.mac.trim();
        let (logical_port, external_mac) = if mac.is_empty() {
            (set([]), set([]))
        } else {
            (set([json!(port.meta.id.to_string())]), set([json!(mac)]))
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

    /// Remove any `NAT` row with `external_ids:pistack:id = <fip id>`.
    async fn delete_fip(&self, id: &Id) -> ApiResult<()> {
        let found = self
            .run(vec![ovsdb::select("NAT", tagged("pistack:id", id))])
            .await?;
        let ops = unhook_nat(&ovsdb::rows(&found[0]));
        if !ops.is_empty() {
            self.run(ops).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ovsdb::fake::FakeDb;
    use crate::types::{
        FloatingIp, FloatingIpSpec, FloatingIpStatus, HostRoute, NetworkSpec, NetworkStatus,
        PortSpec, PortStatus, Router, RouterSpec, RouterStatus, SecurityGroupSpec,
        SecurityGroupStatus, StaticRoute, SubnetSpec, SubnetStatus,
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

    fn router_port(net: &Network, subnet: &Subnet, rtr: &Id, mac: &str, ip: &str) -> Port {
        let mut p = port(net, mac, &[ip]);
        p.spec.device = Some(rtr.clone());
        p.spec.subnet = Some(subnet.meta.id.clone());
        p
    }

    /// An external network, a tenant network, and a router joining them
    /// with one static route.
    async fn routed() -> (Arc<FakeDb>, OvnBackend, RouterView) {
        let (db, b) = backend();
        let b = b.with_gateway_chassis(vec!["gw-a".into(), "gw-b".into()]);
        let mut outside = net(NetworkKind::Overlay);
        outside.spec.external = true;
        let esub = subnet(&outside, "203.0.113.0/24", "203.0.113.1");
        let tenant = net(NetworkKind::Overlay);
        let tsub = subnet(&tenant, "10.0.0.0/24", "10.0.0.1");
        b.ensure_network(&outside).await.unwrap();
        b.ensure_network(&tenant).await.unwrap();
        let router = Router {
            meta: Meta::new(kind::ROUTER, "r", tenant.meta.project.clone()),
            spec: RouterSpec {
                external_network: Some(outside.meta.id.clone()),
                interfaces: vec![tsub.meta.id.clone()],
                routes: vec![StaticRoute {
                    destination: "192.168.0.0/16".parse().unwrap(),
                    nexthop: "10.0.0.254".parse().unwrap(),
                }],
            },
            status: RouterStatus::default(),
        };
        let gw = router_port(
            &outside,
            &esub,
            &router.meta.id,
            "fa:16:3e:00:00:01",
            "203.0.113.10",
        );
        let iface = router_port(
            &tenant,
            &tsub,
            &router.meta.id,
            "fa:16:3e:00:00:02",
            "10.0.0.1",
        );
        let view = RouterView {
            router,
            gateway: Some((outside, gw)),
            gateway_subnet: Some(esub),
            interfaces: vec![(tsub, iface)],
        };
        (db, b, view)
    }

    fn named(db: &FakeDb, table: &str, name: &str) -> Option<Value> {
        db.rows(table).into_iter().find(|r| r["name"] == name)
    }

    fn route_pairs(db: &FakeDb) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = db
            .rows("Logical_Router_Static_Route")
            .iter()
            .map(|r| {
                (
                    r["ip_prefix"].as_str().unwrap().to_string(),
                    r["nexthop"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        out.sort();
        out
    }

    fn nat_of(db: &FakeDb, kind: &str) -> Vec<Value> {
        db.rows("NAT")
            .into_iter()
            .filter(|r| r["type"] == kind)
            .collect()
    }

    #[tokio::test]
    async fn a_router_gets_ports_peers_gateway_routes_and_snat() {
        let (db, b, view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        let rid = &view.router.meta.id;
        let lr = named(&db, "Logical_Router", &format!("pr-{rid}")).unwrap();
        assert_eq!(
            opt(&lr, "options", "dynamic_neigh_routers").as_deref(),
            Some("true")
        );

        let (tsub, iface) = &view.interfaces[0];
        let lrp = named(
            &db,
            "Logical_Router_Port",
            &format!("lrp-{}", iface.meta.id),
        )
        .unwrap();
        assert_eq!(lrp["mac"], "fa:16:3e:00:00:02");
        assert_eq!(ovsdb::canon(&lrp["networks"]), json!("10.0.0.1/24"));
        let peer = named(&db, "Logical_Switch_Port", &iface.meta.id.to_string()).unwrap();
        assert_eq!(peer["type"], "router");
        assert_eq!(
            opt(&peer, "options", "router-port"),
            Some(format!("lrp-{}", iface.meta.id))
        );
        let ls = named(&db, "Logical_Switch", &switch_name(&tsub.spec.network)).unwrap();
        assert!(ovsdb::elems(ls.get("ports")).contains(&peer["_uuid"]));

        let (_, gw) = view.gateway.as_ref().unwrap();
        let glrp = named(&db, "Logical_Router_Port", &format!("lrp-{}", gw.meta.id)).unwrap();
        assert_eq!(ovsdb::canon(&glrp["networks"]), json!("203.0.113.10/24"));
        let mut chassis: Vec<(String, i64)> = db
            .rows("Gateway_Chassis")
            .iter()
            .map(|r| {
                (
                    r["chassis_name"].as_str().unwrap().to_string(),
                    r["priority"].as_i64().unwrap(),
                )
            })
            .collect();
        chassis.sort();
        assert_eq!(chassis, vec![("gw-a".into(), 2), ("gw-b".into(), 1)]);
        assert_eq!(ovsdb::elems(glrp.get("gateway_chassis")).len(), 2);

        assert_eq!(
            route_pairs(&db),
            vec![
                ("0.0.0.0/0".into(), "203.0.113.1".into()),
                ("192.168.0.0/16".into(), "10.0.0.254".into()),
            ]
        );
        let snat = nat_of(&db, "snat");
        assert_eq!(snat.len(), 1);
        assert_eq!(snat[0]["logical_ip"], "10.0.0.0/24");
        assert_eq!(snat[0]["external_ip"], "203.0.113.10");

        let writes = db.writes();
        b.ensure_router(&view).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat writes nothing");
    }

    #[tokio::test]
    async fn dropping_an_interface_and_a_route_removes_only_those() {
        let (db, b, mut view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        let (_, iface) = view.interfaces.pop().unwrap();
        view.router.spec.interfaces.clear();
        view.router.spec.routes.clear();
        b.ensure_router(&view).await.unwrap();

        assert_eq!(db.rows("Logical_Router_Port").len(), 1);
        assert!(named(&db, "Logical_Switch_Port", &iface.meta.id.to_string()).is_none());
        assert_eq!(
            route_pairs(&db),
            vec![("0.0.0.0/0".into(), "203.0.113.1".into())]
        );
        assert!(nat_of(&db, "snat").is_empty());
        assert_eq!(db.rows("Gateway_Chassis").len(), 2);
    }

    #[tokio::test]
    async fn a_gateway_without_its_subnet_waits() {
        let (_db, b, mut view) = routed().await;
        view.gateway_subnet = None;
        let err = b.ensure_router(&view).await.unwrap_err();
        assert!(err.message.contains("has no subnet"));
    }

    #[tokio::test]
    async fn deleting_a_router_takes_its_rows_and_switch_peers() {
        let (db, b, view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        b.delete_router(&view.router.meta.id).await.unwrap();
        for table in [
            "Logical_Router",
            "Logical_Router_Port",
            "Logical_Router_Static_Route",
            "NAT",
            "Gateway_Chassis",
        ] {
            assert!(db.rows(table).is_empty(), "{table} left behind");
        }
        assert!(db
            .rows("Logical_Switch_Port")
            .iter()
            .all(|r| r["type"] != "router"));
        assert_eq!(db.rows("Logical_Switch").len(), 2);
        b.delete_router(&view.router.meta.id).await.unwrap();
    }

    #[tokio::test]
    async fn a_floating_ip_is_one_nat_row_that_follows_its_port() {
        let (db, b, view) = routed().await;
        b.ensure_router(&view).await.unwrap();
        let (outside, _) = view.gateway.clone().unwrap();
        let tenant_net = {
            let (tsub, _) = &view.interfaces[0];
            let mut n = net(NetworkKind::Overlay);
            n.meta.id = tsub.spec.network.clone();
            n.meta.project = tsub.meta.project.clone();
            n
        };
        let vm = port(&tenant_net, "fa:16:3e:00:00:05", &["10.0.0.5", "10.0.0.6"]);
        b.ensure_port(&tenant_net, &vm, &[]).await.unwrap();
        let fip = FloatingIp {
            meta: Meta::new(kind::FLOATING_IP, "f", tenant_net.meta.project.clone()),
            spec: FloatingIpSpec {
                network: outside.meta.id.clone(),
                address: Some("203.0.113.20".parse().unwrap()),
                port: Some(vm.meta.id.clone()),
            },
            status: FloatingIpStatus::default(),
        };
        let at = |ip: &str| FipView {
            fip: fip.clone(),
            target: Some((vm.clone(), ip.parse().unwrap())),
            router: Some(view.router.clone()),
        };

        b.associate_fip(&at("10.0.0.5")).await.unwrap();
        let dnat = nat_of(&db, "dnat_and_snat");
        assert_eq!(dnat.len(), 1);
        assert_eq!(dnat[0]["external_ip"], "203.0.113.20");
        assert_eq!(dnat[0]["logical_ip"], "10.0.0.5");
        assert_eq!(
            ovsdb::canon(&dnat[0]["logical_port"]),
            json!(vm.meta.id.to_string())
        );
        assert_eq!(
            ovsdb::canon(&dnat[0]["external_mac"]),
            json!("fa:16:3e:00:00:05")
        );
        let lr = db.rows("Logical_Router").pop().unwrap();
        assert!(ovsdb::elems(lr.get("nat")).contains(&dnat[0]["_uuid"]));

        let writes = db.writes();
        b.associate_fip(&at("10.0.0.5")).await.unwrap();
        b.ensure_router(&view).await.unwrap();
        assert_eq!(db.writes(), writes, "repeats write nothing");
        assert_eq!(
            nat_of(&db, "dnat_and_snat").len(),
            1,
            "router keeps the fip row"
        );

        b.associate_fip(&at("10.0.0.6")).await.unwrap();
        let dnat = nat_of(&db, "dnat_and_snat");
        assert_eq!(dnat.len(), 1);
        assert_eq!(dnat[0]["logical_ip"], "10.0.0.6");

        let off = FipView {
            fip: fip.clone(),
            target: None,
            router: None,
        };
        b.associate_fip(&off).await.unwrap();
        assert!(nat_of(&db, "dnat_and_snat").is_empty());
        assert_eq!(nat_of(&db, "snat").len(), 1, "router snat stays");

        b.associate_fip(&at("10.0.0.5")).await.unwrap();
        b.delete_fip(&fip.meta.id).await.unwrap();
        assert!(nat_of(&db, "dnat_and_snat").is_empty());
        b.delete_fip(&fip.meta.id).await.unwrap();

        let lost = FipView {
            router: None,
            ..at("10.0.0.5")
        };
        let err = b.associate_fip(&lost).await.unwrap_err();
        assert!(err.message.contains("no router joins"));
    }
    fn sg(rules: Vec<Rule>) -> SecurityGroup {
        SecurityGroup {
            meta: Meta::new(kind::SECGROUP, "sg", Some(Id::new(kind::PROJECT))),
            spec: SecurityGroupSpec { rules },
            status: SecurityGroupStatus::default(),
        }
    }

    fn group(db: &FakeDb, name: &str) -> Option<Value> {
        db.rows("Port_Group")
            .into_iter()
            .find(|r| r["name"] == name)
    }

    fn acl_matches(db: &FakeDb, group: &str) -> Vec<String> {
        let mut m: Vec<String> = db
            .rows("ACL")
            .iter()
            .filter(|r| ext(r, "pistack:group").as_deref() == Some(group))
            .map(|r| {
                format!(
                    "{} {} {} {}",
                    r["direction"].as_str().unwrap(),
                    r["priority"],
                    r["action"].as_str().unwrap(),
                    r["match"].as_str().unwrap()
                )
            })
            .collect();
        m.sort();
        m
    }

    async fn ready_port(b: &OvnBackend, groups: &[SecurityGroup]) -> (Network, Port) {
        let n = net(NetworkKind::Overlay);
        b.ensure_network(&n).await.unwrap();
        let mut p = port(&n, "fa:16:3e:00:00:10", &["10.20.0.10"]);
        p.spec.security_groups = groups.iter().map(|g| g.meta.id.clone()).collect();
        b.ensure_port(&n, &p, groups).await.unwrap();
        (n, p)
    }

    fn lsp_uuid(db: &FakeDb, p: &Port) -> Value {
        db.rows("Logical_Switch_Port")
            .into_iter()
            .find(|r| r["name"] == json!(p.meta.id.to_string()))
            .map(|r| r["_uuid"].clone())
            .unwrap()
    }

    #[tokio::test]
    async fn rules_become_acls_on_the_groups_port_group() {
        let (db, b) = backend();
        let other = Id::new(kind::SECGROUP);
        let mut g = sg(vec![]);
        let me = g.meta.id.clone();
        g.spec.rules = vec![
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(22, 22)
                .remote(Remote::Cidr("10.0.0.0/8".parse().unwrap())),
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Udp)
                .ports(5000, 5100),
            Rule::new(Direction::Ingress, EtherType::IPv4).remote(Remote::Group(me.clone())),
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .protocol(Protocol::Icmp)
                .remote(Remote::Group(other.clone())),
            Rule::new(Direction::Egress, EtherType::IPv4),
            Rule::new(Direction::Egress, EtherType::IPv6),
            // A v4 remote on an IPv6 rule matches nothing.
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .remote(Remote::Cidr("192.168.0.0/16".parse().unwrap())),
        ];
        b.apply_security_group(&g, &[]).await.unwrap();
        let name = format!("pg_{}", me.uuid.simple());
        let o = format!("pg_{}", other.uuid.simple());
        let mut want = vec![
            format!("from-lport 1002 allow-related inport == @{name} && ip4"),
            format!("from-lport 1002 allow-related inport == @{name} && ip6"),
            format!("to-lport 1002 allow-related outport == @{name} && ip4 && ip4.src == ${name}_ip4"),
            format!("to-lport 1002 allow-related outport == @{name} && ip4 && ip4.src == 10.0.0.0/8 && tcp && tcp.dst == 22"),
            format!("to-lport 1002 allow-related outport == @{name} && ip4 && udp && udp.dst >= 5000 && udp.dst <= 5100"),
            format!("to-lport 1002 allow-related outport == @{name} && ip6 && ip6.src == ${o}_ip6 && icmp6"),
        ];
        want.sort();
        assert_eq!(acl_matches(&db, &name), want);
        let pg = group(&db, &name).unwrap();
        assert_eq!(ext(&pg, "pistack:id"), Some(me.to_string()));
        assert_eq!(ovsdb::elems(pg.get("acls")).len(), 6);
        // The group the ICMP rule names exists, empty, so its address set does.
        let og = group(&db, &o).unwrap();
        assert!(ovsdb::elems(og.get("acls")).is_empty());
        assert_eq!(ext(&og, "pistack:id"), Some(other.to_string()));

        let writes = db.writes();
        b.apply_security_group(&g, &[]).await.unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        g.spec.rules.truncate(1);
        b.apply_security_group(&g, &[]).await.unwrap();
        assert_eq!(acl_matches(&db, &name).len(), 1);
        assert_eq!(db.rows("ACL").len(), 1, "replaced ACLs are dropped");

        b.delete_security_group(&me).await.unwrap();
        b.delete_security_group(&me).await.unwrap();
        assert!(group(&db, &name).is_none());
        assert!(db.rows("ACL").is_empty());
    }

    #[tokio::test]
    async fn a_port_with_security_joins_the_drop_group_and_its_groups() {
        let (db, b) = backend();
        let web = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        let db_sg = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        let (n, mut p) = ready_port(&b, std::slice::from_ref(&web)).await;
        let u = lsp_uuid(&db, &p);
        let web_name = format!("pg_{}", web.meta.id.uuid.simple());
        let db_name = format!("pg_{}", db_sg.meta.id.uuid.simple());

        let drop = group(&db, "pg_pistack_drop").unwrap();
        assert_eq!(ovsdb::elems(drop.get("ports")), vec![u.clone()]);
        let mut want = vec![
            "from-lport 1001 drop inport == @pg_pistack_drop && ip".to_string(),
            "to-lport 1001 drop outport == @pg_pistack_drop && ip".to_string(),
            "from-lport 1002 allow inport == @pg_pistack_drop && udp && udp.src == 68 && udp.dst == 67".to_string(),
            "from-lport 1002 allow inport == @pg_pistack_drop && udp && udp.src == 546 && udp.dst == 547".to_string(),
        ];
        want.sort();
        assert_eq!(acl_matches(&db, "pg_pistack_drop"), want);
        let wg = group(&db, &web_name).unwrap();
        assert_eq!(ovsdb::elems(wg.get("ports")), vec![u.clone()]);
        assert_eq!(acl_matches(&db, &web_name).len(), 1);

        let writes = db.writes();
        b.ensure_port(&n, &p, std::slice::from_ref(&web))
            .await
            .unwrap();
        assert_eq!(db.writes(), writes, "a repeat must not write");

        // Applying the group with its members agrees with the port.
        b.apply_security_group(&web, std::slice::from_ref(&p))
            .await
            .unwrap();
        assert_eq!(db.writes(), writes);

        // Moving the port to another group takes it out of the first.
        p.spec.security_groups = vec![db_sg.meta.id.clone()];
        b.ensure_port(&n, &p, std::slice::from_ref(&db_sg))
            .await
            .unwrap();
        assert!(ovsdb::elems(group(&db, &web_name).unwrap().get("ports")).is_empty());
        assert_eq!(
            ovsdb::elems(group(&db, &db_name).unwrap().get("ports")),
            vec![u.clone()]
        );

        // Without port security it is in no group at all.
        p.spec.port_security = false;
        p.spec.security_groups.clear();
        b.ensure_port(&n, &p, &[]).await.unwrap();
        for g in db.rows("Port_Group") {
            assert!(ovsdb::elems(g.get("ports")).is_empty(), "{g}");
        }

        // Deleting a port takes it out of its groups.
        p.spec.port_security = true;
        p.spec.security_groups = vec![db_sg.meta.id.clone()];
        b.ensure_port(&n, &p, std::slice::from_ref(&db_sg))
            .await
            .unwrap();
        b.delete_port(&p.meta.id).await.unwrap();
        for g in db.rows("Port_Group") {
            assert!(ovsdb::elems(g.get("ports")).is_empty(), "{g}");
        }
    }

    #[tokio::test]
    async fn applying_a_group_sets_its_members_from_their_switch_ports() {
        let (db, b) = backend();
        let g = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        let n = net(NetworkKind::Overlay);
        b.ensure_network(&n).await.unwrap();
        let mut a = port(&n, "fa:16:3e:00:00:21", &["10.20.0.21"]);
        a.spec.security_groups = vec![g.meta.id.clone()];
        b.ensure_port(&n, &a, &[]).await.unwrap();
        let mut c = port(&n, "fa:16:3e:00:00:22", &["10.20.0.22"]);
        c.spec.security_groups = vec![g.meta.id.clone()];
        // `c` has no switch port yet; it joins when it is ensured.
        b.apply_security_group(&g, &[a.clone(), c.clone()])
            .await
            .unwrap();
        let name = format!("pg_{}", g.meta.id.uuid.simple());
        assert_eq!(
            ovsdb::elems(group(&db, &name).unwrap().get("ports")),
            vec![lsp_uuid(&db, &a)]
        );
        b.ensure_port(&n, &c, std::slice::from_ref(&g))
            .await
            .unwrap();
        let mut have = ovsdb::elems(group(&db, &name).unwrap().get("ports"));
        let mut want = vec![lsp_uuid(&db, &a), lsp_uuid(&db, &c)];
        have.sort_by_key(|v| v.to_string());
        want.sort_by_key(|v| v.to_string());
        assert_eq!(have, want);
    }

    #[tokio::test]
    async fn a_follower_writes_no_security_groups() {
        let (db, b) = backend();
        let store = Store::in_memory().unwrap();
        let leadership = Leadership::new(false);
        b.attach(&leadership, &store);
        let g = sg(vec![Rule::new(Direction::Egress, EtherType::IPv4)]);
        assert!(b.apply_security_group(&g, &[]).await.is_err());
        assert!(b.delete_security_group(&g.meta.id).await.is_err());
        assert_eq!(db.writes(), 0);
    }
}
