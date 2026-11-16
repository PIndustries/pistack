// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::backend::ovsdb::fake::FakeDb;
use crate::backend::RouterView;
use crate::types::Rule;
use crate::types::{
    Network, NetworkKind, NetworkSpec, NetworkStatus, Port, PortSpec, PortStatus, Router,
    RouterSpec, RouterStatus, SecurityGroup, SecurityGroupSpec, SecurityGroupStatus, StaticRoute,
    Subnet, SubnetSpec, SubnetStatus,
};
use pistack_types::{kind, Id, Meta, Phase, Resource};
use serde_json::{json, Value};
use std::sync::Arc;

use super::chassis_view::ChassisView;
use super::host_record::HostRecord;
use super::host_status_view::HostStatusView;
use super::host_view::HostView;
use super::ovn_backend::OvnBackend;

pub(in crate::backend::ovn) fn backend() -> (Arc<FakeDb>, OvnBackend) {
    let db = Arc::new(FakeDb::new());
    let b = OvnBackend::with_transport("fake", db.clone());
    (db, b)
}

pub(in crate::backend::ovn) fn net(kind: NetworkKind) -> Network {
    Network {
        meta: Meta::new(kind::NETWORK, "n", Some(Id::new(kind::PROJECT))),
        spec: NetworkSpec {
            kind,
            ..Default::default()
        },
        status: NetworkStatus::default(),
    }
}

pub(in crate::backend::ovn) fn subnet(net: &Network, cidr: &str, gw: &str) -> Subnet {
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

pub(in crate::backend::ovn) fn port(net: &Network, mac: &str, ips: &[&str]) -> Port {
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

/// Switch ports other than the metadata port every network has.
pub(in crate::backend::ovn) fn guest_ports(db: &FakeDb) -> Vec<Value> {
    db.rows("Logical_Switch_Port")
        .into_iter()
        .filter(|r| r["type"] != "localport")
        .collect()
}

pub(in crate::backend::ovn) fn ext(row: &Value, key: &str) -> Option<String> {
    let ps = row["external_ids"][1].as_array()?;
    ps.iter()
        .find(|p| p[0] == key)
        .and_then(|p| p[1].as_str().map(str::to_string))
}

pub(in crate::backend::ovn) fn opt(row: &Value, col: &str, key: &str) -> Option<String> {
    let ps = row[col][1].as_array()?;
    ps.iter()
        .find(|p| p[0] == key)
        .and_then(|p| p[1].as_str().map(str::to_string))
}

pub(in crate::backend::ovn) fn router_port(
    net: &Network,
    subnet: &Subnet,
    rtr: &Id,
    mac: &str,
    ip: &str,
) -> Port {
    let mut p = port(net, mac, &[ip]);
    p.spec.device = Some(rtr.clone());
    p.spec.subnet = Some(subnet.meta.id.clone());
    p
}

/// An external network, a tenant network, and a router joining them
/// with one static route.
pub(in crate::backend::ovn) async fn routed() -> (Arc<FakeDb>, OvnBackend, RouterView) {
    let (db, b) = backend();
    let b = b.with_gateway_chassis(vec!["gw-a".into(), "gw-b".into()]);
    routed_on(db, b).await
}

pub(in crate::backend::ovn) async fn routed_on(
    db: Arc<FakeDb>,
    b: OvnBackend,
) -> (Arc<FakeDb>, OvnBackend, RouterView) {
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
            vpc: None,
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

pub(in crate::backend::ovn) fn host_record(
    chassis: &str,
    phase: Phase,
    ovn: Option<(bool, &str)>,
) -> HostRecord {
    Resource {
        meta: Meta::new(kind::HOST, chassis, None),
        spec: HostView {
            node_name: chassis.into(),
        },
        status: HostStatusView {
            phase,
            ovn: ovn.map(|(ready, mappings)| ChassisView {
                chassis: chassis.into(),
                ready,
                bridge_mappings: mappings.into(),
            }),
        },
    }
}

pub(in crate::backend::ovn) fn scheduled_on(db: &FakeDb) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = db
        .rows("Gateway_Chassis")
        .iter()
        .map(|r| {
            (
                r["chassis_name"].as_str().unwrap().to_string(),
                r["priority"].as_i64().unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

pub(in crate::backend::ovn) fn named(db: &FakeDb, table: &str, name: &str) -> Option<Value> {
    db.rows(table).into_iter().find(|r| r["name"] == name)
}

pub(in crate::backend::ovn) fn route_pairs(db: &FakeDb) -> Vec<(String, String)> {
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

pub(in crate::backend::ovn) fn nat_of(db: &FakeDb, kind: &str) -> Vec<Value> {
    db.rows("NAT")
        .into_iter()
        .filter(|r| r["type"] == kind)
        .collect()
}

pub(in crate::backend::ovn) fn sg(rules: Vec<Rule>) -> SecurityGroup {
    SecurityGroup {
        meta: Meta::new(kind::SECGROUP, "sg", Some(Id::new(kind::PROJECT))),
        spec: SecurityGroupSpec { rules },
        status: SecurityGroupStatus::default(),
    }
}

pub(in crate::backend::ovn) fn group(db: &FakeDb, name: &str) -> Option<Value> {
    db.rows("Port_Group")
        .into_iter()
        .find(|r| r["name"] == name)
}

pub(in crate::backend::ovn) fn acl_matches(db: &FakeDb, group: &str) -> Vec<String> {
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

pub(in crate::backend::ovn) async fn ready_port(
    b: &OvnBackend,
    groups: &[SecurityGroup],
) -> (Network, Port) {
    let n = net(NetworkKind::Overlay);
    b.ensure_network(&n).await.unwrap();
    let mut p = port(&n, "fa:16:3e:00:00:10", &["10.20.0.10"]);
    p.spec.security_groups = groups.iter().map(|g| g.meta.id.clone()).collect();
    b.ensure_port(&n, &p, groups).await.unwrap();
    (n, p)
}

use crate::backend::NetBackend;

pub(in crate::backend::ovn) fn lsp_uuid(db: &FakeDb, p: &Port) -> Value {
    db.rows("Logical_Switch_Port")
        .into_iter()
        .find(|r| r["name"] == json!(p.meta.id.uuid.to_string()))
        .map(|r| r["_uuid"].clone())
        .unwrap()
}
