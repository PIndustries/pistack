// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Port and floating-IP quotas. No project, or no limit for the resource,
//! means unlimited. A missing project row is the same: callers address ids
//! that are not gate projects. A disabled project rejects a create.
//!
//! Ports whose device is a router — the gateway and the interface ports
//! fabric creates for that router — are not counted. There is no separate
//! DHCP port. A normal `create_port` is counted. Usage is recorded after
//! the row is stored and released when that row is deleted, once.

use gate::{check_quota, record_usage, Project};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id};

const PORTS: &str = "ports";
const FLOATING_IPS: &str = "floating_ips";

pub fn admit_port(store: &Store, project: Option<&Id>) -> ApiResult<()> {
    admit(store, project, PORTS)
}

pub fn note_port_create(store: &Store, project: Option<&Id>) {
    adjust(store, project, PORTS, 1);
}

pub fn note_port_delete(store: &Store, project: Option<&Id>) {
    adjust(store, project, PORTS, -1);
}

pub fn admit_floating_ip(store: &Store, project: Option<&Id>) -> ApiResult<()> {
    admit(store, project, FLOATING_IPS)
}

pub fn note_floating_ip_create(store: &Store, project: Option<&Id>) {
    adjust(store, project, FLOATING_IPS, 1);
}

pub fn note_floating_ip_delete(store: &Store, project: Option<&Id>) {
    adjust(store, project, FLOATING_IPS, -1);
}

fn admit(store: &Store, project: Option<&Id>, resource: &str) -> ApiResult<()> {
    let Some(project) = project else {
        return Ok(());
    };
    match check_quota(store, project, resource, 1) {
        Err(e) if e.code == ErrorCode::NotFound => Ok(()),
        other => other,
    }
}

fn adjust(store: &Store, project: Option<&Id>, resource: &str, delta: i64) {
    let Some(project) = project else {
        return;
    };
    match store.collection::<Project>().try_get(project) {
        Ok(None) => return,
        Ok(Some(_)) => {}
        Err(e) => {
            tracing::warn!(%project, resource, delta, error = %e, "fabric: usage was not recorded");
            return;
        }
    }
    if let Err(e) = record_usage(store, project, resource, delta) {
        tracing::warn!(%project, resource, delta, error = %e, "fabric: usage was not recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Client, FloatingIpSpec, MockBackend, NetworkSpec, PortSpec, RouterSpec, SubnetSpec,
    };
    use gate::{ProjectSpec, ProjectStatus};
    use pistack_types::{kind, Meta};
    use std::sync::Arc;

    fn harness() -> (Store, Client) {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
        (store, client)
    }

    fn tenant(store: &Store, name: &str, resource: &str, limit: Option<u64>) -> Id {
        let mut spec = ProjectSpec::default();
        if let Some(limit) = limit {
            spec.quotas.insert(resource.into(), limit);
        }
        store
            .collection::<Project>()
            .create(Project {
                meta: Meta::new(kind::PROJECT, name, None),
                spec,
                status: ProjectStatus::default(),
            })
            .unwrap()
            .meta
            .id
    }

    fn used(store: &Store, project: &Id, resource: &str) -> u64 {
        store
            .collection::<Project>()
            .get(project)
            .unwrap()
            .status
            .usage
            .get(resource)
            .copied()
            .unwrap_or(0)
    }

    fn network(client: &Client, project: &Id, name: &str, cidr: &str, gw: &str) -> (Id, Id) {
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, name, Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        let sub = client
            .create_subnet(
                Meta::new(kind::SUBNET, name, Some(project.clone())),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: cidr.parse().unwrap(),
                    gateway: Some(gw.parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap();
        (net.meta.id, sub.meta.id)
    }

    fn external(client: &Client, project: &Id) -> Id {
        let spec = NetworkSpec {
            external: true,
            ..Default::default()
        };
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "public", Some(project.clone())),
                spec,
            )
            .unwrap();
        client
            .create_subnet(
                Meta::new(kind::SUBNET, "public", Some(project.clone())),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: "203.0.113.0/24".parse().unwrap(),
                    gateway: Some("203.0.113.1".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: false,
                    host_routes: vec![],
                },
            )
            .unwrap();
        net.meta.id
    }

    fn create_port(client: &Client, project: &Id, net: &Id, name: &str) -> ApiResult<crate::Port> {
        client.create_port(
            Meta::new(kind::PORT, name, Some(project.clone())),
            PortSpec {
                network: net.clone(),
                ..PortSpec::default()
            },
        )
    }

    #[test]
    fn a_zero_port_quota_refuses_and_leaves_no_row() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", PORTS, Some(0));
        let (net, _) = network(&client, &project, "app", "10.8.0.0/24", "10.8.0.1");
        let err = create_port(&client, &project, &net, "web").unwrap_err();
        assert_eq!(err.code, ErrorCode::QuotaExceeded);
        assert!(client.ports().list(Some(&project)).unwrap().is_empty());
        assert_eq!(used(&store, &project, PORTS), 0);
    }

    #[test]
    fn no_port_quota_creates_and_delete_releases_once() {
        let (store, client) = harness();
        let project = tenant(&store, "open", PORTS, None);
        let (net, _) = network(&client, &project, "app", "10.8.0.0/24", "10.8.0.1");
        let first = create_port(&client, &project, &net, "web").unwrap();
        assert_eq!(client.ports().list(Some(&project)).unwrap().len(), 1);
        assert_eq!(used(&store, &project, PORTS), 1);

        client.delete_port(&first.meta.id).unwrap();
        assert_eq!(used(&store, &project, PORTS), 0);
        assert!(client.delete_port(&first.meta.id).is_err());
        assert_eq!(used(&store, &project, PORTS), 0);
        assert!(client.ports().list(Some(&project)).unwrap().is_empty());

        let second = create_port(&client, &project, &net, "web").unwrap();
        assert_eq!(used(&store, &project, PORTS), 1);
        client.delete_port(&second.meta.id).unwrap();
        assert_eq!(used(&store, &project, PORTS), 0);
    }

    #[test]
    fn router_gateway_and_interface_ports_are_not_counted() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", PORTS, Some(0));
        let (_net, sub) = network(&client, &project, "app", "10.8.0.0/24", "10.8.0.1");
        let ext = external(&client, &project);
        let router = client
            .create_router(
                Meta::new(kind::ROUTER, "edge", Some(project.clone())),
                RouterSpec {
                    external_network: Some(ext),
                    interfaces: vec![sub],
                    routes: vec![],
                },
            )
            .unwrap();
        let ports = client.ports().list(Some(&project)).unwrap();
        assert_eq!(ports.len(), 2, "gateway plus one interface");
        assert!(ports.iter().all(|p| p.is_router_port()));
        assert_eq!(used(&store, &project, PORTS), 0);

        let err = create_port(&client, &project, &ports[0].spec.network, "user").unwrap_err();
        assert_eq!(err.code, ErrorCode::QuotaExceeded);
        assert_eq!(client.ports().list(Some(&project)).unwrap().len(), 2);

        client.delete_router(&router.meta.id).unwrap();
        assert!(client.ports().list(Some(&project)).unwrap().is_empty());
        assert_eq!(used(&store, &project, PORTS), 0);
    }

    #[test]
    fn floating_ip_quota_matches_ports() {
        let (store, client) = harness();
        let tight = tenant(&store, "tight", FLOATING_IPS, Some(0));
        let ext = external(&client, &tight);
        let err = client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "a", Some(tight.clone())),
                FloatingIpSpec {
                    network: ext.clone(),
                    address: None,
                    port: None,
                },
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::QuotaExceeded);
        assert!(client.floating_ips().list(Some(&tight)).unwrap().is_empty());
        assert_eq!(used(&store, &tight, FLOATING_IPS), 0);

        let open = tenant(&store, "open", FLOATING_IPS, None);
        let ext = external(&client, &open);
        let fip = client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "a", Some(open.clone())),
                FloatingIpSpec {
                    network: ext,
                    address: None,
                    port: None,
                },
            )
            .unwrap();
        assert_eq!(client.floating_ips().list(Some(&open)).unwrap().len(), 1);
        assert_eq!(used(&store, &open, FLOATING_IPS), 1);
        client.delete_floating_ip(&fip.meta.id).unwrap();
        assert_eq!(used(&store, &open, FLOATING_IPS), 0);
        assert!(client.delete_floating_ip(&fip.meta.id).is_err());
        assert_eq!(used(&store, &open, FLOATING_IPS), 0);
        assert!(client.floating_ips().list(Some(&open)).unwrap().is_empty());
    }
}
