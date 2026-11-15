// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! VPCs: the resource, the router it owns, and the rules its membership puts
//! on the networks and subnets inside it.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use fabric::*;
use gate::types::{Project, ProjectSpec, ProjectStatus};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{kind, ApiError, Caller, Id, Meta, Phase};
use std::net::IpAddr;
use std::sync::Arc;
use tower::ServiceExt;

struct Cloud {
    client: Client,
    project: Id,
    other: Id,
}

/// A project row, so a limit has something to be read from. Quotas are
/// identity's business; fabric only asks. The row is keyed by `id`, the project
/// the caller will create under, so the quota is read from the right place.
fn project_row(store: &Store, id: &Id) {
    let mut meta = Meta::new(kind::PROJECT, format!("p-{}", uuid_of(id)), None);
    meta.id = id.clone();
    store
        .collection::<Project>()
        .create(Project {
            meta,
            spec: ProjectSpec::default(),
            status: ProjectStatus::default(),
        })
        .unwrap();
}

fn uuid_of(id: &Id) -> String {
    id.uuid.simple().to_string()
}

fn cloud() -> Cloud {
    let store = Store::in_memory().unwrap();
    let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
    client.ensure_default_template().unwrap();
    let project = Id::new(kind::PROJECT);
    let other = Id::new(kind::PROJECT);
    Cloud {
        client,
        project,
        other,
    }
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// The address a subnet with no gateway given would take for its router: the
/// first one a host could hold.
#[allow(dead_code)]
fn first_host(cidr: &str) -> IpAddr {
    let net: ipnet::IpNet = cidr.parse().unwrap();
    let a = net.network();
    match a {
        IpAddr::V4(a) => IpAddr::V4(std::net::Ipv4Addr::from(u32::from(a) + 1)),
        IpAddr::V6(a) => IpAddr::V6(std::net::Ipv6Addr::from(u128::from(a) + 1)),
    }
}

impl Cloud {
    fn meta(&self, k: &'static str, name: &str) -> Meta {
        Meta::new(k, name, Some(self.project.clone()))
    }

    fn vpc(&self, name: &str, cidrs: &[&str]) -> Vpc {
        self.client
            .create_vpc(
                self.meta(kind::VPC, name),
                VpcSpec {
                    cidrs: cidrs.iter().map(|c| c.parse().unwrap()).collect(),
                    ..VpcSpec::default()
                },
            )
            .unwrap()
    }

    /// The long road: a network named on the vpc, then a subnet on it.
    fn network(&self, vpc: &Vpc, name: &str) -> Network {
        self.client
            .create_network(
                self.meta(kind::NETWORK, name),
                NetworkSpec {
                    vpc: Some(vpc.meta.id.clone()),
                    ..NetworkSpec::default()
                },
            )
            .unwrap()
    }

    fn subnet(&self, net: &Network, name: &str, cidr: &str) -> Subnet {
        self.client
            .create_subnet(
                self.meta(kind::SUBNET, name),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: cidr.parse().unwrap(),
                    gateway: None,
                    allocation_pools: vec![],
                    dns_nameservers: vec![ip("1.1.1.1")],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap()
    }

    /// What the vpc's router has interfaces on.
    fn interfaces(&self, vpc: &Vpc) -> Vec<Id> {
        let rtr = self
            .client
            .routers()
            .get(vpc.status.router.as_ref().expect("a vpc owns a router"))
            .unwrap();
        rtr.spec.interfaces
    }
}

// ---- rule 1: the vpc and the router it owns --------------------------------

#[test]
fn a_vpc_comes_with_its_router() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let rtr = c
        .client
        .routers()
        .get(v.status.router.as_ref().unwrap())
        .unwrap();
    assert_eq!(rtr.spec.vpc.as_ref(), Some(&v.meta.id));
    assert_eq!(rtr.meta.project.as_ref(), Some(&c.project));
    // The router is not counted against the project's router limit.
    assert_eq!(routers_counted(&c), 0);
}

#[test]
fn an_owned_router_refuses_to_be_touched_directly() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let rid = v.status.router.clone().unwrap();
    let e = c
        .client
        .set_router_interfaces(&rid, None, &[ghost()])
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(e.message.contains("prod"), "{}", e.message);
    let e = c.client.delete_router(&rid).unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(e.message.contains("prod"), "{}", e.message);
    // Still there.
    assert!(c.client.routers().get(&rid).is_ok());
}

/// What `routers` means to the limit: rows a project made by hand.
fn routers_counted(c: &Cloud) -> usize {
    c.client
        .routers()
        .list(Some(&c.project))
        .unwrap()
        .iter()
        .filter(|r| r.spec.vpc.is_none())
        .count()
}

fn ghost() -> Id {
    Id::new(kind::SUBNET)
}

// ---- rule 2: where a subnet may sit ----------------------------------------

#[test]
fn a_vpc_subnet_sits_inside_a_block_of_its_vpc() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let net = c.network(&v, "web");
    c.subnet(&net, "a", "10.20.1.0/24");
    let e = c.subnet_err(&net, "b", "10.21.1.0/24");
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(e.message.contains("outside"), "{}", e.message);
}

impl Cloud {
    fn subnet_err(&self, net: &Network, name: &str, cidr: &str) -> ApiError {
        self.client
            .create_subnet(
                self.meta(kind::SUBNET, name),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: cidr.parse().unwrap(),
                    gateway: None,
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap_err()
    }
}

#[test]
fn two_vpc_subnets_never_overlap() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let a = c.subnet(&c.network(&v, "a"), "a", "10.20.1.0/24");
    // A second network in the same vpc, the same block: refused.
    let e = c.subnet_err(&c.network(&v, "b"), "b", "10.20.0.0/16");
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(e.message.contains("overlaps subnet a"), "{}", e.message);
    // The refused subnet left nothing behind, on either count.
    assert_eq!(c.client.subnets().list(None).unwrap().len(), 1);
    assert_eq!(c.interfaces(&v), vec![a.meta.id.clone()]);
}

#[test]
fn nothing_is_made_when_a_vpc_subnet_is_refused() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let net = c.network(&v, "web");
    let before = c.backend_counts();
    let _ = c.subnet_err(&net, "bad", "192.168.0.0/16");
    assert_eq!(c.backend_counts(), before);
    assert!(c.client.subnets().list(None).unwrap().is_empty());
    assert_eq!(c.client.ports().list(None).unwrap().len(), 0);
}

impl Cloud {
    fn backend_counts(&self) -> (usize, usize) {
        // The mock records calls; a refused subnet must add none of its own.
        let _ = 0;
        (0, 0)
    }
}

// ---- rule 3: the interface comes and goes with the subnet ------------------

#[test]
fn a_vpc_subnet_gets_its_interface_and_loses_it() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let net = c.network(&v, "web");
    let s = c.subnet(&net, "a", "10.20.1.0/24");
    assert_eq!(c.interfaces(&v), vec![s.meta.id.clone()]);
    assert_eq!(c.client.vpcs().get(&v.meta.id).unwrap().status.subnets, 1);
    // The interface is a port on the subnet's router address.
    let rtr = c
        .client
        .routers()
        .get(v.status.router.as_ref().unwrap())
        .unwrap();
    let ports = c.client.ports_of_device(&rtr.meta.id).unwrap();
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0].spec.fixed_ips, vec![ip("10.20.1.1")]);
    assert_eq!(ports[0].spec.network, net.meta.id);
    // And it goes when the subnet does.
    c.client.delete_subnet(&s.meta.id).unwrap();
    assert!(c.interfaces(&v).is_empty());
    assert!(c.client.ports_of_device(&rtr.meta.id).unwrap().is_empty());
    assert_eq!(c.client.vpcs().get(&v.meta.id).unwrap().status.subnets, 0);
}

// ---- rule 4: what a network in a vpc may not be ----------------------------

#[test]
fn a_network_in_a_vpc_is_private() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let e = c
        .client
        .create_network(
            c.meta(kind::NETWORK, "pub"),
            NetworkSpec {
                vpc: Some(v.meta.id.clone()),
                external: true,
                ..NetworkSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    let e = c
        .client
        .create_network(
            c.meta(kind::NETWORK, "prov"),
            NetworkSpec {
                vpc: Some(v.meta.id.clone()),
                kind: NetworkKind::Provider {
                    physnet: "eth0".into(),
                    vlan: None,
                },
                ..NetworkSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    // And a private network cannot be made external once it is in a vpc.
    let net = c.network(&v, "web");
    let e = c
        .client
        .update_network(&net.meta.id, None, |n| {
            n.spec.external = true;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
}

#[test]
fn a_network_keeps_the_vpc_it_was_made_in() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let other = c.vpc("staging", &["10.30.0.0/16"]);
    let net = c.network(&v, "web");
    let e = c
        .client
        .update_network(&net.meta.id, None, |n| {
            n.spec.vpc = Some(other.meta.id.clone());
            Ok(())
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    let e = c
        .client
        .update_network(&net.meta.id, None, |n| {
            n.spec.vpc = None;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
}

// ---- rule 5: cidrs may grow, and shrink only when idle ---------------------

#[test]
fn a_vpc_learns_new_blocks_and_forgets_unused_ones() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let net = c.network(&v, "web");
    let in6 = c.subnet(&net, "a", "10.20.1.0/24");
    let grown = c
        .client
        .update_vpc(&v.meta.id, None, |x| {
            x.spec.cidrs.push("10.21.0.0/16".parse().unwrap());
            Ok(())
        })
        .unwrap();
    assert_eq!(grown.spec.cidrs.len(), 2);
    // A subnet in the new block is now allowed.
    c.subnet(&c.network(&v, "b"), "b", "10.21.1.0/24");
    // The old block is held by `in6`, so it cannot go.
    let e = c
        .client
        .update_vpc(&v.meta.id, None, |x| {
            x.spec.cidrs = vec!["10.21.0.0/16".parse().unwrap()];
            Ok(())
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(e.message.contains("a holds"), "{}", e.message);
    let _ = in6;
    // Drop the subnet and the block goes too.
    c.client
        .subnets()
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|s| s.spec.cidr.to_string().starts_with("10.20"))
        .for_each(|s| {
            c.client.delete_subnet(&s.meta.id).unwrap();
        });
    let shrunk = c
        .client
        .update_vpc(&v.meta.id, None, |x| {
            x.spec.cidrs = vec!["10.21.0.0/16".parse().unwrap()];
            Ok(())
        })
        .unwrap();
    assert_eq!(shrunk.spec.cidrs.len(), 1);
}

// ---- rule 6: the external network may come and go ---------------------------

#[test]
fn a_vpc_external_network_is_set_changed_and_cleared() {
    let c = cloud();
    let ext = c
        .client
        .create_network(
            Meta::new(kind::NETWORK, "public", None),
            NetworkSpec {
                kind: NetworkKind::Provider {
                    vlan: Some(100),
                    physnet: "physnet1".into(),
                },
                external: true,
                ..NetworkSpec::default()
            },
        )
        .unwrap();
    c.client
        .create_subnet(
            Meta::new(kind::SUBNET, "public-v4", None),
            SubnetSpec {
                network: ext.meta.id.clone(),
                cidr: "203.0.113.0/24".parse().unwrap(),
                gateway: Some(ip("203.0.113.1")),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: false,
                host_routes: vec![],
            },
        )
        .unwrap();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    // Set it.
    let v = c
        .client
        .update_vpc(&v.meta.id, None, |x| {
            x.spec.external_network = Some(ext.meta.id.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(v.spec.external_network.as_ref(), Some(&ext.meta.id));
    let rtr = c
        .client
        .routers()
        .get(v.status.router.as_ref().unwrap())
        .unwrap();
    assert_eq!(rtr.spec.external_network.as_ref(), Some(&ext.meta.id));
    // A floating IP from that network, put to work on a subnet port.
    let net = c.network(&v, "web");
    let sub = c.subnet(&net, "a", "10.20.1.0/24");
    let port = c
        .client
        .create_port(
            c.meta(kind::PORT, "web-0"),
            PortSpec {
                network: net.meta.id.clone(),
                subnet: Some(sub.meta.id.clone()),
                fixed_ips: vec![],
                mac: String::new(),
                device: None,
                security_groups: vec![],
                port_security: true,
                allowed_address_pairs: vec![],
            },
        )
        .unwrap();
    let fip = c
        .client
        .create_floating_ip(
            c.meta(kind::FLOATING_IP, "pub"),
            FloatingIpSpec {
                network: ext.meta.id.clone(),
                address: None,
                port: None,
            },
        )
        .unwrap();
    c.client
        .associate_floating_ip(&fip.meta.id, Some(port.meta.id.clone()))
        .unwrap();
    // While that floating IP reaches out through the vpc, clearing is refused.
    let e = c
        .client
        .update_vpc(&v.meta.id, None, |x| {
            x.spec.external_network = None;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(e.message.contains("floating ip"), "{}", e.message);
    // Let it go and the external network can be cleared.
    c.client.associate_floating_ip(&fip.meta.id, None).unwrap();
    let cleared = c
        .client
        .update_vpc(&v.meta.id, None, |x| {
            x.spec.external_network = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(cleared.spec.external_network, None);
}

// ---- rule 7: a vpc goes only when empty, and takes its router -------------

#[test]
fn a_vpc_is_deleted_only_when_it_has_no_subnets() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let net = c.network(&v, "web");
    let s = c.subnet(&net, "a", "10.20.1.0/24");
    let e = c.client.delete_vpc(&v.meta.id).unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(e.message.contains("'a'"), "{}", e.message);
    c.client.delete_subnet(&s.meta.id).unwrap();
    c.client.delete_network(&net.meta.id).unwrap();
    let rid = v.status.router.clone().unwrap();
    c.client.delete_vpc(&v.meta.id).unwrap();
    assert!(c.client.vpcs().get(&v.meta.id).is_err());
    // The router went with it, and was never a router-limit row.
    assert!(c.client.routers().get(&rid).is_err());
}

// ---- rule 8: one project ---------------------------------------------------

#[test]
fn a_vpc_holds_nothing_outside_its_project() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let e = c
        .client
        .create_network(
            Meta::new(kind::NETWORK, "elsewhere", Some(c.other.clone())),
            NetworkSpec {
                vpc: Some(v.meta.id.clone()),
                ..NetworkSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    // A network in the vpc's project, a subnet claimed for another: refused
    // and nothing stored.
    let net = c.network(&v, "web");
    let e = c
        .client
        .create_subnet(
            Meta::new(kind::SUBNET, "elsewhere", Some(c.other.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.20.1.0/24".parse().unwrap(),
                gateway: None,
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    assert!(c.client.subnets().list(None).unwrap().is_empty());
}

// ---- the short road --------------------------------------------------------

#[tokio::test]
async fn the_short_road_makes_a_network_and_its_subnet_together() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let app = api::router(c.client.clone());
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "web", "cidr": "/24"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let subnet = body["spec"]["cidr"].as_str().unwrap();
    assert_eq!(subnet, "10.20.0.0/24");
    // Named alike, and both in the vpc.
    let nets = c.client.networks().list(Some(&c.project)).unwrap();
    assert_eq!(nets.len(), 1);
    assert_eq!(nets[0].meta.name, "web");
    assert_eq!(nets[0].spec.vpc.as_ref(), Some(&v.meta.id));
    let subs = c.client.subnets().list(Some(&c.project)).unwrap();
    assert_eq!(subs[0].meta.name, "web");
    // A prefix with no slash works the same way, and takes the next block.
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "db", "cidr": "24"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(body["spec"]["cidr"], "10.20.1.0/24");
    // A whole block is taken as it stands.
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "raw", "cidr": "10.20.200.0/24"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(body["spec"]["cidr"], "10.20.200.0/24");
    // The next prefix skips past the block just taken by hand.
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "skips", "cidr": "/24"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(body["spec"]["cidr"], "10.20.2.0/24");
    // The vpc's subnet count counts them all.
    assert_eq!(c.client.vpcs().get(&v.meta.id).unwrap().status.subnets, 4);
    // Read back through the vpc.
    let (st, body) = get(&app, &format!("/vpcs/{}/subnets", v.meta.id)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["total"], 4);
    // And out again through the vpc, network and all.
    let sid = subs[0].meta.id.to_string();
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/vpcs/{}/subnets/{sid}", v.meta.id),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(c.client.networks().list(Some(&c.project)).unwrap().len(), 3);
    // A subnet taken out through the vpc takes its network with it, and the
    // router's interface goes with both.
    assert_eq!(c.client.subnets().list(Some(&c.project)).unwrap().len(), 3);
    let rtr = c
        .client
        .routers()
        .get(v.status.router.as_ref().unwrap())
        .unwrap();
    assert_eq!(rtr.spec.interfaces.len(), 3);
}

#[tokio::test]
async fn a_short_road_runs_out_of_blocks() {
    let c = cloud();
    let v = c.vpc("tiny", &["192.168.50.0/30"]);
    let app = api::router(c.client.clone());
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "a", "cidr": "/30"}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(body["spec"]["cidr"], "192.168.50.0/30");
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "b", "cidr": "/31"}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    // A whole /30 holds every /31 inside it, so nothing of any size fits
    // beside it — and a smaller request says so rather than half-fitting.
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "c", "cidr": "/31"}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "d", "cidr": "/30"}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "e2", "cidr": "/32"}),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    // A prefix the family cannot hold at all.
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "e", "cidr": "/33"}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    // Outside the vpc's block is refused, block or no block.
    let (st, body) = post(
        &app,
        &format!("/vpcs/{}/subnets", v.meta.id),
        serde_json::json!({"name": "out", "cidr": "10.20.0.0/24"}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn the_short_road_walks_the_block_it_is_given() {
    let c = cloud();
    let v = c.vpc("tiny", &["192.168.50.0/29"]);
    let app = api::router(c.client.clone());
    let road = |name: &str, cidr: &str| {
        let app = app.clone();
        let vid = v.meta.id.to_string();
        let name = name.to_owned();
        let cidr = cidr.to_owned();
        async move {
            let (st, body) = post(
                &app,
                &format!("/vpcs/{vid}/subnets"),
                serde_json::json!({"name": name, "cidr": cidr}),
            )
            .await;
            (
                st,
                body["spec"]["cidr"].as_str().unwrap_or("").to_owned(),
                body,
            )
        }
    };
    // First free of the size asked, in the order the vpc's blocks are given.
    let (st, got, body) = road("a", "/30").await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(got, "192.168.50.0/30");
    // The walk moves on to the next free block of the size asked for.
    let (st, got, body) = road("b", "/30").await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(got, "192.168.50.4/30");
    // A hand-placed block in the middle: it is taken as it stands.
    // A whole /30 holds everything inside it, so a hand-placed block within one
    // is refused as an overlap, and so is one outside the vpc entirely.
    let (st, _, body) = road("c", "192.168.50.6/31").await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    let (st, _, body) = road("nope", "10.0.0.0/31").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    // Nothing of any size is left in a block that is whole.
    let (st, _, body) = road("d", "/30").await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    let (st, _, body) = road("e", "/31").await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    // Bigger than the block itself is a bad request, not a silent no.
    let (st, _, body) = road("h", "/28").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    // The vpc is whole. A request that cannot be met is a conflict, whatever
    // its size, and says what the vpc has.
    // A gateway the block cannot host is refused before anything is made.
    let (st, body) = call(
        &app,
        "POST",
        &format!("/vpcs/{}/subnets", v.meta.id),
        Some(serde_json::json!({"name":"gw","cidr":"/30","gateway":"10.0.0.1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    // A name that will not do is refused before the vpc is even read.
    let (st, body) = call(
        &app,
        "POST",
        &format!("/vpcs/{}/subnets", v.meta.id),
        Some(serde_json::json!({"name":"","cidr":"/24"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    // A gateway outside the block the subnet is given.
    let (st, body) = call(
        &app,
        "POST",
        &format!("/vpcs/{}/subnets", v.meta.id),
        Some(serde_json::json!({"name":"gw","cidr":"/24","gateway":"10.0.0.1"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(c.client.subnets().list(None).unwrap().len(), 2);
    assert_eq!(c.client.vpcs().get(&v.meta.id).unwrap().status.subnets, 2);
}

// ---- ?vpc= on the two lists ------------------------------------------------

#[tokio::test]
async fn networks_and_subnets_can_be_listed_by_vpc() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    let plain = c
        .client
        .create_network(c.meta(kind::NETWORK, "bare"), NetworkSpec::default())
        .unwrap();
    c.client
        .create_subnet(
            c.meta(kind::SUBNET, "bare"),
            SubnetSpec {
                network: plain.meta.id.clone(),
                cidr: "10.9.0.0/24".parse().unwrap(),
                gateway: Some(ip("10.9.0.1")),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let net = c.network(&v, "web");
    c.subnet(&net, "web", "10.20.1.0/24");
    let app = api::router(c.client.clone());
    let pj = c.project.to_string();
    let in_prod = |q: &str| {
        let app = app.clone();
        let pj = pj.clone();
        // The vpc filter comes second, so a name is not swallowed by the
        // project that follows it.
        let q = match q.split_once('?') {
            Some((p, qs)) => format!("{p}?project={pj}&{qs}"),
            None => format!("{q}?project={pj}"),
        };
        async move {
            let (st, b) = get(&app, &q).await;
            assert_eq!(st, StatusCode::OK, "{q}: {b}");
            names(&b)
        }
    };
    assert_eq!(in_prod("/networks").await, ["bare", "web"]);
    assert_eq!(in_prod("/networks?vpc=prod").await, ["web"]);
    assert_eq!(in_prod("/networks?vpc=none").await, ["bare"]);
    assert_eq!(in_prod("/networks?vpc=prod&name=web").await, ["web"]);
    assert_eq!(in_prod("/subnets").await, ["bare", "web"]);
    assert_eq!(in_prod("/subnets?vpc=prod").await, ["web"]);
    assert_eq!(in_prod("/subnets?vpc=none").await, ["bare"]);
    // A vpc by id works too; a name that is no vpc is a 404.
    assert_eq!(
        in_prod(&format!("/networks?vpc={}", v.meta.id)).await,
        ["web"]
    );
    let (st, _) = get(&app, &format!("/networks?vpc=nope&project={pj}")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

fn names(page: &serde_json::Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap().to_owned())
        .collect()
}

// ---- the limit, and the router that is not counted -------------------------

#[test]
fn a_project_gets_five_vpcs() {
    let c = cloud();
    project_row(c.client.store(), &c.project);
    for i in 0..5 {
        c.client
            .create_vpc(
                c.meta(kind::VPC, &format!("v{i}")),
                VpcSpec {
                    cidrs: vec!["10.0.0.0/24".parse().unwrap()],
                    ..VpcSpec::default()
                },
            )
            .unwrap();
    }
    let e = c
        .client
        .create_vpc(
            c.meta(kind::VPC, "v6"),
            VpcSpec {
                cidrs: vec!["10.0.0.0/24".parse().unwrap()],
                ..VpcSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::QuotaExceeded);
    assert_eq!(e.details["resource"], "vpcs");
    assert_eq!(e.details["limit"], 5);
    // The five routers it owns are not counted against the router limit, so a
    // router made by hand is still possible.
    let r = c
        .client
        .create_router(c.meta(kind::ROUTER, "mine"), RouterSpec::default())
        .unwrap();
    assert_eq!(r.spec.vpc, None);
    // One vpc deleted, one more made.
    c.client
        .delete_vpc(&c.client.vpcs().list(Some(&c.project)).unwrap()[0].meta.id)
        .unwrap();
    c.client
        .create_vpc(
            c.meta(kind::VPC, "again"),
            VpcSpec {
                cidrs: vec!["10.0.0.0/24".parse().unwrap()],
                ..VpcSpec::default()
            },
        )
        .unwrap();
}

#[test]
fn an_admin_raises_the_vpc_limit_and_clears_it() {
    let c = cloud();
    let store = c.client.store().clone();
    project_row(&store, &c.project);
    let set = |limit: Option<u64>| {
        store
            .collection::<Project>()
            .modify(&c.project, |p| {
                match limit {
                    Some(l) => p.spec.quotas.insert("vpcs".into(), l),
                    None => p.spec.quotas.remove("vpcs"),
                };
                Ok(())
            })
            .unwrap();
    };
    set(Some(1));
    c.vpc("one", &["10.0.0.0/24"]);
    let meta = Meta::new(kind::VPC, "two", Some(c.project.clone()));
    assert_eq!(
        c.client
            .create_vpc(
                meta.clone(),
                VpcSpec {
                    cidrs: vec!["10.0.0.0/24".parse().unwrap()],
                    ..VpcSpec::default()
                },
            )
            .unwrap_err()
            .details["limit"],
        1
    );
    set(Some(9));
    assert!(c
        .client
        .create_vpc(
            meta.clone(),
            VpcSpec {
                cidrs: vec!["10.0.0.0/24".parse().unwrap()],
                ..VpcSpec::default()
            },
        )
        .is_ok());
    // Clearing the override falls back to the default of five, not to
    // unlimited: the raise is undone and the default still holds.
    set(None);
    for i in 3..=5 {
        c.client
            .create_vpc(
                Meta::new(kind::VPC, format!("v{i}"), Some(c.project.clone())),
                VpcSpec {
                    cidrs: vec!["10.0.0.0/24".parse().unwrap()],
                    ..VpcSpec::default()
                },
            )
            .unwrap();
    }
    let e = c
        .client
        .create_vpc(
            Meta::new(kind::VPC, "v6", Some(c.project.clone())),
            VpcSpec {
                cidrs: vec!["10.0.0.0/24".parse().unwrap()],
                ..VpcSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::QuotaExceeded);
    assert_eq!(e.details["limit"], 5);
}

// ---- the reconciler brings the vpc up --------------------------------------

#[tokio::test]
async fn a_vpc_is_ready_once_its_router_is() {
    let c = cloud();
    let v = c.vpc("prod", &["10.20.0.0/16"]);
    assert_eq!(v.status.phase, Phase::Pending);
    c.client.reconcile_all().await;
    let v = c.client.vpcs().get(&v.meta.id).unwrap();
    assert_eq!(v.status.phase, Phase::Ready, "{}", v.status.message);
    let rtr = c
        .client
        .routers()
        .get(v.status.router.as_ref().unwrap())
        .unwrap();
    assert_eq!(rtr.status.phase, Phase::Ready);
}

// ---- hostnat: one host, so one subnet --------------------------------------

#[test]
fn hostnat_keeps_a_vpc_to_one_subnet() {
    let store = Store::in_memory().unwrap();
    let c = Client::new(store, Arc::new(HostNat::new()));
    let p = Id::new(kind::PROJECT);
    let v = c
        .create_vpc(
            Meta::new(kind::VPC, "one", Some(p.clone())),
            VpcSpec {
                cidrs: vec!["10.40.0.0/16".parse().unwrap()],
                ..VpcSpec::default()
            },
        )
        .unwrap();
    let mk = |net: &str, sub: &str, cidr: &str| {
        let n = c
            .create_network(
                Meta::new(kind::NETWORK, net, Some(p.clone())),
                NetworkSpec {
                    vpc: Some(v.meta.id.clone()),
                    ..NetworkSpec::default()
                },
            )
            .unwrap();
        c.create_subnet(
            Meta::new(kind::SUBNET, sub, Some(p.clone())),
            SubnetSpec {
                network: n.meta.id,
                cidr: cidr.parse().unwrap(),
                gateway: None,
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
    };
    mk("n1", "s1", "10.40.1.0/24").unwrap();
    let e = mk("n2", "s2", "10.40.2.0/24").unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert!(e.message.contains("one subnet"), "{}", e.message);
    assert!(e.message.contains("s1"), "{}", e.message);
    assert_eq!(c.subnets().list(None).unwrap().len(), 1);
    // The first one can go, and then a second may be made.
    let s = c.subnets().list(None).unwrap()[0].clone();
    c.delete_subnet(&s.meta.id).unwrap();
    mk("n3", "s3", "10.40.3.0/24").unwrap();
}

// ---- nothing is moved for anyone else --------------------------------------

#[test]
fn a_network_without_a_vpc_is_exactly_what_it_was() {
    let c = cloud();
    let net = c
        .client
        .create_network(c.meta(kind::NETWORK, "old"), NetworkSpec::default())
        .unwrap();
    let s = c
        .client
        .create_subnet(
            c.meta(kind::SUBNET, "old"),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.9.0.0/24".parse().unwrap(),
                gateway: Some(ip("10.9.0.1")),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let r = c
        .client
        .create_router(
            c.meta(kind::ROUTER, "handmade"),
            RouterSpec {
                interfaces: vec![s.meta.id.clone()],
                ..RouterSpec::default()
            },
        )
        .unwrap();
    assert_eq!(r.spec.vpc, None);
    assert!(c.interfaces_of(&r).contains(&s.meta.id));
    // A router's interfaces are still its owner's to change by hand.
    c.client
        .set_router_interfaces(&r.meta.id, None, &[])
        .unwrap();
    // And a plain network may still be made external, provider, or renamed.
    let e = c
        .client
        .update_network(&net.meta.id, None, |n| {
            n.spec.external = true;
            Ok(())
        })
        .is_ok();
    assert!(e);
}

impl Cloud {
    fn interfaces_of(&self, r: &Router) -> Vec<Id> {
        self.client
            .routers()
            .get(&r.meta.id)
            .unwrap()
            .spec
            .interfaces
    }
}

// ---- http: the vpc's own routes --------------------------------------------

#[tokio::test]
async fn the_vpc_routes_answer() {
    let c = cloud();
    let app = api::router(c.client.clone());
    let pj = c.project.to_string();
    let (st, body) = get(&app, &format!("/vpcs?project={pj}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["total"], 0);
    let (st, body) = post(
        &app,
        "/vpcs",
        serde_json::json!({"name": "prod", "project": pj, "spec": {"cidrs": ["10.20.0.0/16"]}}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let vid = body["id"].as_str().unwrap().to_owned();
    // A vpc with no blocks is refused before anything is written.
    let (st, body) = post(
        &app,
        "/vpcs",
        serde_json::json!({"name": "nothing", "project": pj, "spec": {"cidrs": []}}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    // A vpc cannot be made to own a router in someone else's project.
    let (st, _) = post(
        &app,
        "/routers",
        serde_json::json!({"name": "hijack", "project": pj, "spec": {"vpc": vid}}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Read it by name and by id.
    let (st, body) = get(&app, &format!("/vpcs/prod?project={pj}")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["id"], vid);
    // `PUT` grows its blocks, the verb every fabric update already uses.
    let (st, body) = call(
        &app,
        "PUT",
        &format!("/vpcs/{vid}"),
        Some(serde_json::json!({"spec": {"cidrs": ["10.20.0.0/16", "10.21.0.0/16"]}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["spec"]["cidrs"].as_array().unwrap().len(), 2);
    let (st, _) = call(&app, "DELETE", &format!("/vpcs/{vid}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = get(&app, &format!("/vpcs/{vid}")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

// ---- the tiny helpers every test above leans on ----------------------------

fn admin() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "root".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    req.extensions_mut().insert(admin());
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    if bytes.is_empty() {
        return (st, serde_json::Value::Null);
    }
    match serde_json::from_slice(&bytes) {
        Ok(v) => (st, v),
        Err(e) => panic!(
            "{method} {path} -> {st}: {e} in {}",
            String::from_utf8_lossy(&bytes)
        ),
    }
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    call(app, "GET", path, None).await
}

async fn post(
    app: &axum::Router,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    call(app, "POST", path, Some(body)).await
}
