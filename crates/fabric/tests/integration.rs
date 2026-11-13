// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end tests over `Store::in_memory` and `MockBackend`.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use fabric::backend::Call;
use fabric::*;
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{kind, Caller, Id, Meta, Module, ModuleContext, Phase};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

struct Harness {
    client: Client,
    backend: Arc<MockBackend>,
    project: Id,
}

fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
    let backend = Arc::new(MockBackend::new());
    let client = Client::new(store, backend.clone());
    client.ensure_default_template().unwrap();
    Harness {
        client,
        backend,
        project: Id::new(kind::PROJECT),
    }
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

impl Harness {
    fn meta(&self, k: &'static str, name: &str) -> Meta {
        Meta::new(k, name, Some(self.project.clone()))
    }

    fn network(&self, name: &str) -> Network {
        self.client
            .create_network(self.meta(kind::NETWORK, name), NetworkSpec::default())
            .unwrap()
    }

    fn subnet(&self, net: &Network, name: &str, cidr: &str, gw: &str) -> Subnet {
        self.client
            .create_subnet(
                self.meta(kind::SUBNET, name),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: cidr.parse().unwrap(),
                    gateway: Some(ip(gw)),
                    allocation_pools: vec![],
                    dns_nameservers: vec![ip("1.1.1.1")],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap()
    }

    fn port(&self, net: &Network, name: &str) -> Port {
        self.client
            .create_port(
                self.meta(kind::PORT, name),
                PortSpec {
                    network: net.meta.id.clone(),
                    ..PortSpec::default()
                },
            )
            .unwrap()
    }

    /// A cloud-wide external network with one subnet.
    fn external(&self) -> (Network, Subnet) {
        let mut meta = Meta::new(kind::NETWORK, "public", None);
        meta.description = "provider".into();
        let net = self
            .client
            .create_network(
                meta,
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
        let sub = self
            .client
            .create_subnet(
                Meta::new(kind::SUBNET, "public-v4", None),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: "203.0.113.0/24".parse().unwrap(),
                    gateway: Some(ip("203.0.113.1")),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: false,
                    host_routes: vec![],
                },
            )
            .unwrap();
        (net, sub)
    }
}

#[tokio::test]
async fn network_subnet_ports_router_fip_flow() {
    let h = harness();
    let c = &h.client;

    let net = h.network("app");
    let sub = h.subnet(&net, "app-v4", "10.0.0.0/24", "10.0.0.1");
    let p1 = h.port(&net, "web-0");
    let p2 = h.port(&net, "web-1");

    // Distinct, deterministic addresses; gateway skipped.
    assert_eq!(p1.spec.fixed_ips, vec![ip("10.0.0.2")]);
    assert_eq!(p2.spec.fixed_ips, vec![ip("10.0.0.3")]);
    assert_ne!(p1.spec.mac, p2.spec.mac);
    assert!(p1.spec.mac.starts_with("02:70:69:"));
    assert_eq!(p1.spec.subnet, Some(sub.meta.id.clone()));

    // Default security group was cloned for the project and attached.
    let default_sg = c.default_security_group(&h.project).unwrap();
    assert_eq!(default_sg.meta.name, DEFAULT_SG);
    assert_eq!(p1.spec.security_groups, vec![default_sg.meta.id.clone()]);
    assert_eq!(default_sg.spec.rules.len(), 4);
    assert!(default_sg
        .spec
        .rules
        .iter()
        .any(|r| r.direction == Direction::Ingress
            && r.remote == Remote::Group(default_sg.meta.id.clone())));
    // Cloning is idempotent.
    assert_eq!(
        c.default_security_group(&h.project).unwrap().meta.id,
        default_sg.meta.id
    );

    // External network, router with gateway + interface.
    let (ext, _ext_sub) = h.external();
    let router = c
        .create_router(
            h.meta(kind::ROUTER, "edge"),
            RouterSpec {
                external_network: Some(ext.meta.id.clone()),
                interfaces: vec![],
                routes: vec![],
            },
        )
        .unwrap();
    let router = c
        .set_router_interfaces(&router.meta.id, None, std::slice::from_ref(&sub.meta.id))
        .unwrap();
    assert_eq!(router.spec.interfaces, vec![sub.meta.id.clone()]);
    let rports = c.ports_of_device(&router.meta.id).unwrap();
    assert_eq!(rports.len(), 2, "gateway port + interface port");
    let iface = rports
        .iter()
        .find(|p| p.spec.network == net.meta.id)
        .unwrap();
    assert_eq!(
        iface.spec.fixed_ips,
        vec![ip("10.0.0.1")],
        "interface port holds the gateway"
    );
    let gw = rports
        .iter()
        .find(|p| p.spec.network == ext.meta.id)
        .unwrap();
    assert_eq!(gw.spec.fixed_ips, vec![ip("203.0.113.2")]);

    // Floating IP: allocated from the external subnet, skipping the router's address.
    let fip = c
        .create_floating_ip(
            h.meta(kind::FLOATING_IP, "web-0-public"),
            FloatingIpSpec {
                network: ext.meta.id.clone(),
                address: None,
                port: None,
            },
        )
        .unwrap();
    assert_eq!(fip.spec.address, Some(ip("203.0.113.3")));
    let fip = c
        .associate_floating_ip(&fip.meta.id, Some(p1.meta.id.clone()))
        .unwrap();
    assert_eq!(fip.spec.port, Some(p1.meta.id.clone()));
    // A port holds one floating IP at a time.
    let fip2 = c
        .create_floating_ip(
            h.meta(kind::FLOATING_IP, "dup"),
            FloatingIpSpec {
                network: ext.meta.id.clone(),
                address: None,
                port: Some(p1.meta.id.clone()),
            },
        )
        .unwrap_err();
    assert_eq!(fip2.code, ErrorCode::Conflict);

    // Nothing reached the backend yet; the reconciler does that.
    assert!(h.backend.calls().is_empty());
    c.reconcile_all().await;

    let calls = h.backend.calls();
    assert!(calls.contains(&Call::EnsureNetwork(net.meta.id.clone())));
    assert!(calls.contains(&Call::EnsureNetwork(ext.meta.id.clone())));
    assert!(calls.contains(&Call::EnsureSubnet {
        network: net.meta.id.clone(),
        subnet: sub.meta.id.clone()
    }));
    assert!(calls.iter().any(|c| matches!(c, Call::EnsurePort { port, addresses, groups, .. }
        if *port == p1.meta.id && *addresses == vec![ip("10.0.0.2")] && *groups == vec![default_sg.meta.id.clone()])));
    assert!(calls
        .iter()
        .any(|c| matches!(c, Call::EnsurePort { port, .. } if *port == p2.meta.id)));
    assert!(calls.contains(&Call::EnsureRouter {
        router: router.meta.id.clone(),
        gateway: Some(gw.meta.id.clone()),
        interfaces: vec![sub.meta.id.clone()],
    }));
    assert!(calls.contains(&Call::AssociateFip {
        fip: fip.meta.id.clone(),
        port: Some(p1.meta.id.clone()),
        fixed_ip: Some(ip("10.0.0.2")),
    }));
    // Network before its subnet, subnet's network before ports.
    let pos = |pred: &dyn Fn(&Call) -> bool| calls.iter().position(pred).unwrap();
    assert!(
        pos(&|c| *c == Call::EnsureNetwork(net.meta.id.clone()))
            < pos(&|c| matches!(c, Call::EnsureSubnet { .. }))
    );
    assert!(
        pos(&|c| matches!(c, Call::EnsureSubnet { .. }))
            < pos(&|c| matches!(c, Call::EnsurePort { .. }))
    );
    // The last security-group application saw both user ports as members.
    let last_sg = calls.iter().rev().find_map(|c| match c {
        Call::ApplySecurityGroup { sg, members, .. } if *sg == default_sg.meta.id => {
            Some(members.clone())
        }
        _ => None,
    });
    let mut members = last_sg.unwrap();
    members.sort();
    let mut want = vec![p1.meta.id.clone(), p2.meta.id.clone()];
    want.sort();
    assert_eq!(members, want);

    // Status written by the reconciler.
    let net = c.networks().get(&net.meta.id).unwrap();
    assert_eq!(net.status.phase, Phase::Ready);
    assert!(net.status.segment_id.is_some());
    assert_eq!(net.status.observed_revision, net.meta.revision);
    let ext = c.networks().get(&ext.meta.id).unwrap();
    assert_eq!(
        ext.status.segment_id,
        Some(100),
        "provider vlan is the segment"
    );
    let sub = c.subnets().get(&sub.meta.id).unwrap();
    assert_eq!(sub.status.phase, Phase::Ready);
    assert_eq!(sub.status.allocated, 3, "two ports + router interface");
    let p1 = c.ports().get(&p1.meta.id).unwrap();
    assert_eq!(p1.status.phase, Phase::Ready);
    assert_eq!(p1.status.binding.as_ref().unwrap().vif_type, "mock");
    let router = c.routers().get(&router.meta.id).unwrap();
    assert_eq!(router.status.phase, Phase::Ready);
    assert_eq!(router.status.external_ip, Some(ip("203.0.113.2")));
    assert_eq!(router.status.gateway_port, Some(gw.meta.id.clone()));
    let fip = c.floating_ips().get(&fip.meta.id).unwrap();
    assert_eq!(fip.status.phase, Phase::Ready);
    assert_eq!(fip.status.fixed_ip, Some(ip("10.0.0.2")));
    assert_eq!(fip.status.router, Some(router.meta.id.clone()));
    assert_eq!(
        h.backend.fip_target(&fip.meta.id),
        Some(Some(p1.meta.id.clone()))
    );

    // Nothing is stale now, so a pending pass is a no-op on the backend.
    h.backend.clear_calls();
    c.reconcile_pending().await;
    assert!(h.backend.calls().is_empty());

    // Deleting the port detaches the FIP; deleting the router removes its ports.
    c.delete_port(&p1.meta.id).unwrap();
    assert_eq!(c.floating_ips().get(&fip.meta.id).unwrap().spec.port, None);
    assert_eq!(
        c.delete_port(&iface.meta.id).unwrap_err().code,
        ErrorCode::Conflict,
        "router ports are managed"
    );
    assert_eq!(
        c.delete_subnet(&sub.meta.id).unwrap_err().code,
        ErrorCode::Conflict
    );
    c.delete_router(&router.meta.id).unwrap();
    assert!(c.ports_of_device(&router.meta.id).unwrap().is_empty());
    c.delete_port(&p2.meta.id).unwrap();
    c.delete_subnet(&sub.meta.id).unwrap();
    assert_eq!(
        c.delete_network(&ext.meta.id).unwrap_err().code,
        ErrorCode::Conflict,
        "still has a subnet"
    );
    c.delete_network(&net.meta.id).unwrap();
}

#[tokio::test]
async fn fip_without_router_is_an_error_phase() {
    let h = harness();
    let net = h.network("isolated");
    h.subnet(&net, "v4", "10.9.0.0/24", "10.9.0.1");
    let p = h.port(&net, "p");
    let (ext, _) = h.external();
    let fip = h
        .client
        .create_floating_ip(
            h.meta(kind::FLOATING_IP, "f"),
            FloatingIpSpec {
                network: ext.meta.id.clone(),
                address: Some(ip("203.0.113.50")),
                port: Some(p.meta.id.clone()),
            },
        )
        .unwrap();
    h.client.reconcile_all().await;
    let fip = h.client.floating_ips().get(&fip.meta.id).unwrap();
    assert_eq!(fip.status.phase, Phase::Error);
    assert!(fip.status.message.contains("no router"));
    assert!(fip.stale(), "errors are retried by the pending pass");

    // The write that recorded the failure is an event too. Acting on it
    // would fail again, write again and never stop.
    let recorded = fip.meta.revision;
    let event = pistack_types::Event {
        kind: kind::FLOATING_IP.to_owned(),
        id: fip.meta.id.clone(),
        project: fip.meta.project.clone(),
        action: pistack_types::EventAction::Updated,
        revision: recorded,
        at: fip.meta.updated_at,
    };
    for _ in 0..3 {
        h.client.handle_event(&event).await;
    }
    let after = h.client.floating_ips().get(&fip.meta.id).unwrap();
    assert_eq!(after.meta.revision, recorded, "an event must not retry");

    // The pending pass retries, and the same failure is not written again.
    h.client.reconcile_pending().await;
    let after = h.client.floating_ips().get(&fip.meta.id).unwrap();
    assert_eq!(after.status.phase, Phase::Error);
    assert_eq!(after.meta.revision, recorded, "nothing new to record");

    // A router makes the next pass succeed.
    let sub = h.client.subnets().list(None).unwrap();
    let inner = sub.iter().find(|s| s.meta.name == "v4").unwrap();
    h.client
        .create_router(
            h.meta(kind::ROUTER, "r"),
            RouterSpec {
                external_network: Some(ext.meta.id.clone()),
                interfaces: vec![inner.meta.id.clone()],
                ..Default::default()
            },
        )
        .unwrap();
    h.client.reconcile_pending().await;
    let after = h.client.floating_ips().get(&fip.meta.id).unwrap();
    assert_eq!(after.status.phase, Phase::Ready, "{}", after.status.message);
}

#[tokio::test]
async fn subnet_exhaustion_through_ports() {
    let h = harness();
    let net = h.network("tiny");
    // /30: network, gateway, one usable, broadcast.
    h.subnet(&net, "v4", "10.1.0.0/30", "10.1.0.1");
    let p = h.port(&net, "only");
    assert_eq!(p.spec.fixed_ips, vec![ip("10.1.0.2")]);
    let err = h
        .client
        .create_port(
            h.meta(kind::PORT, "nope"),
            PortSpec {
                network: net.meta.id.clone(),
                ..PortSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(err.message, "subnet full");
    // Explicit addresses are validated too.
    let err = h
        .client
        .create_port(
            h.meta(kind::PORT, "dup"),
            PortSpec {
                network: net.meta.id.clone(),
                fixed_ips: vec![ip("10.1.0.2")],
                ..PortSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    let err = h
        .client
        .create_port(
            h.meta(kind::PORT, "gw"),
            PortSpec {
                network: net.meta.id.clone(),
                fixed_ips: vec![ip("10.1.0.1")],
                ..PortSpec::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest);
}

#[tokio::test]
async fn device_ports_for_forge() {
    let h = harness();
    let net = h.network("app");
    h.subnet(&net, "v4", "10.2.0.0/24", "10.2.0.1");
    let machine = Id::new(kind::MACHINE);

    let p0 = h
        .client
        .create_port_for_device(&h.project, "app", machine.clone())
        .await
        .unwrap();
    assert_eq!(p0.meta.name, format!("{machine}-eth0"));
    assert_eq!(p0.spec.device, Some(machine.clone()));
    assert_eq!(p0.status.phase, Phase::Ready, "realised before returning");
    assert!(p0.status.binding.is_some());
    assert!(h.backend.has_port(&p0.meta.id));

    // By id works too, and the index increments.
    let p1 = h
        .client
        .create_port_for_device(&h.project, &net.meta.id.to_string(), machine.clone())
        .await
        .unwrap();
    assert_eq!(p1.meta.name, format!("{machine}-eth1"));
    assert_ne!(p0.spec.fixed_ips, p1.spec.fixed_ips);

    // A cloud-wide network is reachable by name from any project.
    let (ext, _) = h.external();
    let pe = h
        .client
        .create_port_for_device(&h.project, "public", machine.clone())
        .await
        .unwrap();
    assert_eq!(pe.spec.network, ext.meta.id);
    assert_eq!(pe.meta.project, Some(h.project.clone()));

    let err = h
        .client
        .create_port_for_device(&h.project, "nope", machine.clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    let released = h.client.release_device_ports(&machine).unwrap();
    assert_eq!(released.len(), 3);
    assert!(h.client.ports_of_device(&machine).unwrap().is_empty());
}

#[tokio::test]
async fn crud_with_stale_revision_per_kind() {
    let h = harness();
    let c = &h.client;

    // Network
    let net = h.network("n");
    assert_eq!(c.networks().list(Some(&h.project)).unwrap().len(), 1);
    let updated = c
        .update_network(&net.meta.id, Some(net.meta.revision), |n| {
            n.spec.mtu = 9000;
            Ok(())
        })
        .unwrap();
    assert_eq!(updated.spec.mtu, 9000);
    let err = c
        .update_network(&net.meta.id, Some(net.meta.revision), |_| Ok(()))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    let err = c
        .update_network(&net.meta.id, None, |n| {
            n.spec.kind = NetworkKind::Provider {
                vlan: None,
                physnet: "x".into(),
            };
            Ok(())
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest, "kind is immutable");

    // Subnet
    let sub = h.subnet(&net, "s", "10.3.0.0/24", "10.3.0.1");
    assert_eq!(c.subnets().list(Some(&h.project)).unwrap().len(), 1);
    c.update_subnet(&sub.meta.id, Some(sub.meta.revision), |s| {
        s.spec.dns_nameservers.push(ip("8.8.8.8"));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        c.update_subnet(&sub.meta.id, Some(sub.meta.revision), |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let err = h
        .client
        .create_subnet(
            h.meta(kind::SUBNET, "overlap"),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.3.0.128/25".parse().unwrap(),
                gateway: None,
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict, "overlapping cidr");

    // Port
    let port = h.port(&net, "p");
    assert_eq!(c.ports().list(Some(&h.project)).unwrap().len(), 1);
    c.update_port(&port.meta.id, Some(port.meta.revision), |p| {
        p.spec.port_security = false;
        Ok(())
    })
    .unwrap();
    assert!(
        c.ports()
            .get(&port.meta.id)
            .unwrap()
            .spec
            .security_groups
            .is_empty(),
        "no port security, no groups"
    );
    assert_eq!(
        c.update_port(&port.meta.id, Some(port.meta.revision), |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );

    // Router
    let router = c
        .create_router(h.meta(kind::ROUTER, "r"), RouterSpec::default())
        .unwrap();
    assert_eq!(c.routers().list(Some(&h.project)).unwrap().len(), 1);
    c.update_router(&router.meta.id, Some(router.meta.revision), |r| {
        r.spec.routes.push(StaticRoute {
            destination: "192.168.0.0/16".parse().unwrap(),
            nexthop: ip("10.3.0.254"),
        });
        Ok(())
    })
    .unwrap();
    assert_eq!(
        c.update_router(&router.meta.id, Some(router.meta.revision), |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        c.set_router_interfaces(
            &router.meta.id,
            Some(router.meta.revision),
            std::slice::from_ref(&sub.meta.id)
        )
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );

    // Security group with rules add/remove
    let sg = c
        .create_security_group(
            h.meta(kind::SECGROUP, "web"),
            SecurityGroupSpec {
                rules: vec![Rule::new(Direction::Ingress, EtherType::IPv4)
                    .protocol(Protocol::Tcp)
                    .ports(80, 80)],
            },
        )
        .unwrap();
    assert_eq!(
        c.security_groups().list(Some(&h.project)).unwrap().len(),
        2,
        "web + the cloned default"
    );
    let rule = Rule::new(Direction::Ingress, EtherType::IPv4)
        .protocol(Protocol::Tcp)
        .ports(443, 443)
        .remote(Remote::Cidr("0.0.0.0/0".parse().unwrap()));
    let sg2 = c.add_rule(&sg.meta.id, rule.clone()).unwrap();
    assert_eq!(sg2.spec.rules.len(), 2);
    let sg3 = c.remove_rule(&sg.meta.id, rule.id).unwrap();
    assert_eq!(sg3.spec.rules.len(), 1);
    assert_eq!(
        c.remove_rule(&sg.meta.id, rule.id).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        c.update_security_group(&sg.meta.id, Some(sg.meta.revision), |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let bad = Rule::new(Direction::Ingress, EtherType::IPv6)
        .remote(Remote::Cidr("10.0.0.0/8".parse().unwrap()));
    assert_eq!(
        c.add_rule(&sg.meta.id, bad).unwrap_err().code,
        ErrorCode::BadRequest,
        "family mismatch"
    );
    let bad = Rule::new(Direction::Ingress, EtherType::IPv4).ports(1, 2);
    assert_eq!(
        c.add_rule(&sg.meta.id, bad).unwrap_err().code,
        ErrorCode::BadRequest,
        "ports need tcp/udp"
    );
    // In use → cannot delete.
    c.update_port(&port.meta.id, None, |p| {
        p.spec.port_security = true;
        p.spec.security_groups = vec![sg.meta.id.clone()];
        Ok(())
    })
    .unwrap();
    assert_eq!(
        c.delete_security_group(&sg.meta.id).unwrap_err().code,
        ErrorCode::Conflict
    );
    let template = c.security_groups().get_by_name(None, DEFAULT_SG).unwrap();
    assert_eq!(
        c.delete_security_group(&template.meta.id).unwrap_err().code,
        ErrorCode::Forbidden
    );

    // Floating IP
    let (ext, _) = h.external();
    let fip = c
        .create_floating_ip(
            h.meta(kind::FLOATING_IP, "f"),
            FloatingIpSpec {
                network: ext.meta.id.clone(),
                address: None,
                port: None,
            },
        )
        .unwrap();
    assert_eq!(c.floating_ips().list(Some(&h.project)).unwrap().len(), 1);
    c.update_floating_ip(&fip.meta.id, Some(fip.meta.revision), |f| {
        f.meta.description = "x".into();
        Ok(())
    })
    .unwrap();
    assert_eq!(
        c.update_floating_ip(&fip.meta.id, Some(fip.meta.revision), |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let err = c
        .create_floating_ip(
            h.meta(kind::FLOATING_IP, "bad"),
            FloatingIpSpec {
                network: net.meta.id.clone(),
                address: None,
                port: None,
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest, "not an external network");

    // Deletes
    c.delete_floating_ip(&fip.meta.id).unwrap();
    c.delete_router(&router.meta.id).unwrap();
    c.delete_port(&port.meta.id).unwrap();
    c.delete_security_group(&sg.meta.id).unwrap();
    c.delete_subnet(&sub.meta.id).unwrap();
    c.delete_network(&net.meta.id).unwrap();
    assert!(c.networks().list(Some(&h.project)).unwrap().is_empty());
    assert!(c.ports().list(Some(&h.project)).unwrap().is_empty());
}

#[tokio::test]
async fn backend_error_lands_in_status_and_is_retried() {
    let h = harness();
    let net = h.network("flaky");
    h.backend.fail_next("EnsureNetwork");
    assert_eq!(h.client.reconcile_network(&net.meta.id).await, Phase::Error);
    let n = h.client.networks().get(&net.meta.id).unwrap();
    assert_eq!(n.status.phase, Phase::Error);
    assert!(n.status.message.contains("mock backend"));
    h.client.reconcile_pending().await;
    assert_eq!(
        h.client.networks().get(&net.meta.id).unwrap().status.phase,
        Phase::Ready
    );
}

#[tokio::test]
async fn module_start_reconciles_on_events_and_stops_on_shutdown() {
    let store = Store::in_memory().unwrap();
    let backend = Arc::new(MockBackend::new());
    let module = Fabric::with_backend(backend.clone());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    assert_eq!(module.info().prefix, "network");
    module.start(ctx.clone()).await.unwrap();

    let client = module.client(&ctx);
    // start() seeds the template once leadership is held.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if client
            .security_groups()
            .get_by_name(None, DEFAULT_SG)
            .is_ok()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "leader never seeded the default security group"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let project = Id::new(kind::PROJECT);
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "evt", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let n = client.networks().get(&net.meta.id).unwrap();
        if n.status.phase == Phase::Ready {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "reconciler never ran"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(backend.has_network(&net.meta.id));

    // Deletion reaches the backend too.
    client.delete_network(&net.meta.id).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while backend.has_network(&net.meta.id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "backend delete never ran"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
    backend.clear_calls();
    client
        .create_network(
            Meta::new(kind::NETWORK, "after", Some(project)),
            NetworkSpec::default(),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(backend.calls().is_empty(), "reconciler stopped on shutdown");
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    if_match: Option<u64>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(r) = if_match {
        req = req.header(header::IF_MATCH, r.to_string());
    }
    let mut req = req
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    req.extensions_mut().insert(admin_caller());
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

#[tokio::test]
async fn http_roundtrip() {
    let h = harness();
    let app = api::router(h.client.clone());
    let project = h.project.to_string();

    let (st, info) = call(&app, "GET", "/", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(info["backend"], "mock");

    // Create network (dev-admin caller, project in body).
    let (st, net) = call(
        &app,
        "POST",
        "/networks",
        Some(serde_json::json!({"name": "web", "project": project, "labels": {"tier": "web"}, "spec": {"mtu": 1450}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{net}");
    assert_eq!(net["spec"]["kind"]["type"], "overlay");
    assert_eq!(net["status"]["phase"], "pending");
    let net_id = net["id"].as_str().unwrap().to_owned();
    let rev = net["revision"].as_u64().unwrap();

    // Duplicate name → 409.
    let (st, _) = call(
        &app,
        "POST",
        "/networks",
        Some(serde_json::json!({"name": "web", "project": project, "spec": {}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // List, filter, page.
    let (st, page) = call(
        &app,
        "GET",
        &format!("/networks?project={project}&labels=tier=web"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page["total"], 1);
    let (_, page) = call(
        &app,
        "GET",
        &format!("/networks?project={project}&labels=tier=db"),
        None,
        None,
    )
    .await;
    assert_eq!(page["total"], 0);

    // Get by name within the project, and by id.
    let (st, got) = call(
        &app,
        "GET",
        &format!("/networks/web?project={project}"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["id"], net_id);
    let (st, _) = call(&app, "GET", &format!("/networks/{net_id}"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, err) = call(
        &app,
        "GET",
        "/networks/net-00000000-0000-7000-8000-000000000000",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not_found");

    // Update with matching then stale If-Match.
    let (st, upd) = call(
        &app,
        "PUT",
        &format!("/networks/{net_id}"),
        Some(serde_json::json!({"spec": {"mtu": 1400}})),
        Some(rev),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{upd}");
    assert_eq!(upd["spec"]["mtu"], 1400);
    let (st, _) = call(
        &app,
        "PUT",
        &format!("/networks/{net_id}"),
        Some(serde_json::json!({"spec": {"mtu": 1300}})),
        Some(rev),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = call(
        &app,
        "PUT",
        &format!("/networks/{net_id}"),
        Some(serde_json::json!({"spec": {"mtu": 10}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Subnet, two ports, security group rules, router interfaces, FIP associate.
    let (st, sub) = call(
        &app,
        "POST",
        "/subnets",
        Some(serde_json::json!({"name": "web-v4", "project": project, "spec": {"network": net_id, "cidr": "10.5.0.0/24", "gateway": "10.5.0.1"}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{sub}");
    let sub_id = sub["id"].as_str().unwrap().to_owned();
    let (st, p1) = call(
        &app,
        "POST",
        "/ports",
        Some(serde_json::json!({"name": "p1", "project": project, "spec": {"network": net_id}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{p1}");
    let (_, p2) = call(
        &app,
        "POST",
        "/ports",
        Some(serde_json::json!({"name": "p2", "project": project, "spec": {"network": net_id}})),
        None,
    )
    .await;
    assert_eq!(p1["spec"]["fixed_ips"][0], "10.5.0.2");
    assert_eq!(p2["spec"]["fixed_ips"][0], "10.5.0.3");
    assert_ne!(p1["spec"]["mac"], p2["spec"]["mac"]);
    let p1_id = p1["id"].as_str().unwrap().to_owned();

    let (st, sg) = call(
        &app,
        "POST",
        "/security-groups",
        Some(serde_json::json!({"name": "ssh", "project": project, "spec": {}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{sg}");
    let sg_id = sg["id"].as_str().unwrap().to_owned();
    let (st, sg) = call(
        &app,
        "POST",
        &format!("/security-groups/{sg_id}/rules"),
        Some(serde_json::json!({"direction": "ingress", "protocol": "tcp", "port_range": {"min": 22, "max": 22}, "remote": {"cidr": "0.0.0.0/0"}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{sg}");
    let rule_id = sg["spec"]["rules"][0]["id"].as_str().unwrap().to_owned();
    let (st, sg) = call(
        &app,
        "DELETE",
        &format!("/security-groups/{sg_id}/rules/{rule_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        sg["spec"].get("rules").is_none() || sg["spec"]["rules"].as_array().unwrap().is_empty()
    );

    let (ext, _) = h.external();
    let (st, rtr) = call(
        &app,
        "POST",
        "/routers",
        Some(serde_json::json!({"name": "edge", "project": project, "spec": {"external_network": ext.meta.id.to_string()}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{rtr}");
    let rtr_id = rtr["id"].as_str().unwrap().to_owned();
    let (st, rtr) = call(
        &app,
        "PUT",
        &format!("/routers/{rtr_id}/interfaces"),
        Some(serde_json::json!({"subnets": [sub_id]})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rtr}");
    assert_eq!(rtr["spec"]["interfaces"][0], sub_id);

    let (st, fip) = call(
        &app,
        "POST",
        "/floating-ips",
        Some(serde_json::json!({"name": "p1-public", "project": project, "spec": {"network": ext.meta.id.to_string()}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{fip}");
    let fip_id = fip["id"].as_str().unwrap().to_owned();
    let (st, fip) = call(
        &app,
        "POST",
        &format!("/floating-ips/{fip_id}/associate"),
        Some(serde_json::json!({"port": p1_id})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{fip}");
    assert_eq!(fip["spec"]["port"], p1_id);
    let (st, fip) = call(
        &app,
        "POST",
        &format!("/floating-ips/{fip_id}/disassociate"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(fip["spec"].get("port").is_none());

    // Delete guards and deletes.
    let (st, _) = call(&app, "DELETE", &format!("/networks/{net_id}"), None, None).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/floating-ips/{fip_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, "DELETE", &format!("/routers/{rtr_id}"), None, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    for p in [&p1_id, p2["id"].as_str().unwrap()] {
        let (st, _) = call(&app, "DELETE", &format!("/ports/{p}"), None, None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
    }
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/security-groups/{sg_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, "DELETE", &format!("/subnets/{sub_id}"), None, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, "DELETE", &format!("/networks/{net_id}"), None, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, "GET", &format!("/networks/{net_id}"), None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn http_scopes_non_admin_callers() {
    let h = harness();
    let other = Id::new(kind::PROJECT);
    h.network("mine");
    h.client
        .create_network(
            Meta::new(kind::NETWORK, "theirs", Some(other.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    h.client
        .create_network(
            Meta::new(kind::NETWORK, "shared", None),
            NetworkSpec::default(),
        )
        .unwrap();

    let caller = pistack_types::Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "alice".into(),
        project: Some(h.project.clone()),
        roles: vec!["member".into()],
        system: false,
    };
    let app = api::router(h.client.clone()).layer(axum::Extension(caller));

    let (st, page) = call(&app, "GET", "/networks", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page["total"], 1, "only the caller's project");
    let (st, _) = call(&app, "GET", "/networks/mine", None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(&app, "GET", "/networks/theirs", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/networks?project={other}"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&app, "GET", "/networks?all=true", None, None).await;
    assert_eq!(st, StatusCode::OK, "non-admins ignore all=");
    // Creating in another project is refused; omitting project uses the token's.
    let (st, _) = call(
        &app,
        "POST",
        "/networks",
        Some(serde_json::json!({"name": "x", "project": other.to_string(), "spec": {}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, created) = call(
        &app,
        "POST",
        "/networks",
        Some(serde_json::json!({"name": "x", "spec": {}})),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(created["project"], h.project.to_string());
}

fn admin_caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let h = harness();
    let app = api::router(h.client.clone());
    let req = Request::builder()
        .uri("/networks")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}

#[tokio::test]
async fn controllers_idle_until_leader_then_run() {
    use pistack_types::Leadership;

    let store = Store::in_memory().unwrap();
    let backend = Arc::new(MockBackend::new());
    let module = Fabric::with_backend(backend.clone());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let leadership = Leadership::new(false);
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: leadership.clone(),
    };
    module.start(ctx.clone()).await.unwrap();
    let client = module.client(&ctx);

    // Not leader: no default template, no reconcile writes.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        client
            .security_groups()
            .get_by_name(None, DEFAULT_SG)
            .is_err(),
        "follower must not seed template"
    );

    let project = Id::new(kind::PROJECT);
    let net = client
        .create_network(
            Meta::new(kind::NETWORK, "idle", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !backend.has_network(&net.meta.id),
        "follower must not reconcile"
    );
    assert_ne!(
        client.networks().get(&net.meta.id).unwrap().status.phase,
        Phase::Ready
    );

    // Promote: controllers start and catch up.
    leadership.set(true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ready_sg = client
            .security_groups()
            .get_by_name(None, DEFAULT_SG)
            .is_ok();
        let ready_net = client.networks().get(&net.meta.id).unwrap().status.phase == Phase::Ready;
        if ready_sg && ready_net && backend.has_network(&net.meta.id) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "leader never started controllers"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Demote: further changes stay unreconciled.
    leadership.set(false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let net2 = client
        .create_network(
            Meta::new(kind::NETWORK, "after-demote", Some(project)),
            NetworkSpec::default(),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !backend.has_network(&net2.meta.id),
        "demoted node must idle again"
    );

    shutdown.cancel();
}
