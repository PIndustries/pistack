// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Network quotas: `networks`, `subnets`, `routers`, `security_groups`,
//! `ports` and `floating_ips` per project. No project, or no limit for the
//! resource, means unlimited. A missing project row is the same: callers
//! address ids that are not gate projects. A disabled project rejects a
//! create.
//!
//! Usage is the number of rows the project has, counted at each admission
//! under gate's admission lock and copied to the project for reading.
//! What fabric makes on its own is not counted: the gateway and interface
//! ports of a router, and the project's `default` security group. There is
//! no separate DHCP port.

use crate::client::DEFAULT_SG;
use crate::types::{FloatingIp, Network, Port, Router, SecurityGroup, Subnet};
use gate::quota::{self, Demand};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id};

pub use gate::quota::{FLOATING_IPS, NETWORKS, PORTS, ROUTERS, SECURITY_GROUPS, SUBNETS};

/// How many of `resource` the project has.
pub fn count(store: &Store, project: &Id, resource: &str) -> ApiResult<u64> {
    let p = Some(project);
    let n = match resource {
        NETWORKS => store.collection::<Network>().count(p)?,
        SUBNETS => store.collection::<Subnet>().count(p)?,
        ROUTERS => store.collection::<Router>().count(p)?,
        FLOATING_IPS => store.collection::<FloatingIp>().count(p)?,
        SECURITY_GROUPS => store
            .collection::<SecurityGroup>()
            .list(p)?
            .iter()
            .filter(|g| g.meta.name != DEFAULT_SG)
            .count(),
        PORTS => store
            .collection::<Port>()
            .list(p)?
            .iter()
            .filter(|port| !port.is_router_port())
            .count(),
        _ => 0,
    };
    Ok(n as u64)
}

/// Run `create`, which stores one more `resource` in `project`, unless the
/// project is at its limit. `create` must not come back here: the lock is
/// held while it runs.
pub(crate) fn admitted<T>(
    store: &Store,
    project: Option<&Id>,
    resource: &str,
    create: impl FnOnce() -> ApiResult<T>,
) -> ApiResult<T> {
    let Some(project) = project else {
        return create();
    };
    let _admission = quota::admission();
    let used = count(store, project, resource)?;
    match quota::admit(store, project, &[Demand::new(resource, used, 1)]) {
        Err(e) if e.code == ErrorCode::NotFound => {}
        other => other?,
    }
    let out = create()?;
    quota::mirror_usage(store, project, &[(resource, used + 1)]);
    Ok(out)
}

/// Record the count again after a row of `resource` was deleted.
pub(crate) fn released(store: &Store, project: Option<&Id>, resource: &str) {
    let Some(project) = project else {
        return;
    };
    let _admission = quota::admission();
    match count(store, project, resource) {
        Ok(n) => quota::mirror_usage(store, project, &[(resource, n)]),
        Err(e) => {
            tracing::warn!(%project, resource, error = %e, "fabric: usage was not recorded")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Client, FloatingIpSpec, MockBackend, NetworkSpec, PortSpec, RouterSpec, SecurityGroupSpec,
        SubnetSpec,
    };
    use gate::{Project, ProjectSpec, ProjectStatus};
    use pistack_types::{kind, ApiError, Meta};
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

    fn refused(e: &ApiError, resource: &str, used: u64, limit: u64) {
        assert_eq!(e.code, ErrorCode::QuotaExceeded);
        assert_eq!(
            e.message,
            format!(
                "quota exceeded for {resource} in project 'tight': \
                 {used} used + 1 requested > {limit}"
            )
        );
        assert_eq!(e.details["resource"], resource);
        assert_eq!(e.details["limit"], limit);
        assert_eq!(e.details["used"], used);
        assert_eq!(e.details["requested"], 1);
    }

    fn net(client: &Client, project: &Id, name: &str) -> ApiResult<crate::Network> {
        client.create_network(
            Meta::new(kind::NETWORK, name, Some(project.clone())),
            NetworkSpec::default(),
        )
    }

    fn subnet(client: &Client, project: &Id, net: &Id, name: &str, third: u8) -> ApiResult<Subnet> {
        client.create_subnet(
            Meta::new(kind::SUBNET, name, Some(project.clone())),
            SubnetSpec {
                network: net.clone(),
                cidr: format!("10.8.{third}.0/24").parse().unwrap(),
                gateway: Some(format!("10.8.{third}.1").parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
    }

    fn network(client: &Client, project: &Id, name: &str) -> (Id, Id) {
        let net = net(client, project, name).unwrap();
        let sub = subnet(client, project, &net.meta.id, name, 0).unwrap();
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

    fn router(client: &Client, project: &Id, name: &str) -> ApiResult<Router> {
        client.create_router(
            Meta::new(kind::ROUTER, name, Some(project.clone())),
            RouterSpec {
                external_network: None,
                interfaces: vec![],
                routes: vec![],
            },
        )
    }

    fn group(client: &Client, project: &Id, name: &str) -> ApiResult<SecurityGroup> {
        client.create_security_group(
            Meta::new(kind::SECGROUP, name, Some(project.clone())),
            SecurityGroupSpec { rules: vec![] },
        )
    }

    #[test]
    fn a_zero_port_quota_refuses_and_leaves_no_row() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", PORTS, Some(0));
        let (net, _) = network(&client, &project, "app");
        let err = create_port(&client, &project, &net, "web").unwrap_err();
        refused(&err, PORTS, 0, 0);
        assert!(client.ports().list(Some(&project)).unwrap().is_empty());
        assert_eq!(used(&store, &project, PORTS), 0);
    }

    #[test]
    fn no_port_quota_creates_and_delete_releases_once() {
        let (store, client) = harness();
        let project = tenant(&store, "open", PORTS, None);
        let (net, _) = network(&client, &project, "app");
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
        let (_net, sub) = network(&client, &project, "app");
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
        refused(&err, FLOATING_IPS, 0, 0);
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

    #[test]
    fn networks_are_limited_and_a_delete_frees_one() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", NETWORKS, Some(2));
        let first = net(&client, &project, "a").unwrap();
        net(&client, &project, "b").unwrap();
        assert_eq!(used(&store, &project, NETWORKS), 2);
        let err = net(&client, &project, "c").unwrap_err();
        refused(&err, NETWORKS, 2, 2);
        assert_eq!(client.networks().list(Some(&project)).unwrap().len(), 2);

        client.delete_network(&first.meta.id).unwrap();
        assert_eq!(used(&store, &project, NETWORKS), 1);
        net(&client, &project, "c").unwrap();
        assert_eq!(used(&store, &project, NETWORKS), 2);

        let open = tenant(&store, "open", NETWORKS, None);
        for i in 0..5 {
            net(&client, &open, &format!("n{i}")).unwrap();
        }
        assert_eq!(used(&store, &open, NETWORKS), 5);
    }

    #[test]
    fn subnets_are_limited_and_a_delete_frees_one() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", SUBNETS, Some(2));
        let n = net(&client, &project, "app").unwrap().meta.id;
        let first = subnet(&client, &project, &n, "a", 1).unwrap();
        subnet(&client, &project, &n, "b", 2).unwrap();
        assert_eq!(used(&store, &project, SUBNETS), 2);
        let err = subnet(&client, &project, &n, "c", 3).unwrap_err();
        refused(&err, SUBNETS, 2, 2);
        assert_eq!(client.subnets().list(Some(&project)).unwrap().len(), 2);

        client.delete_subnet(&first.meta.id).unwrap();
        assert_eq!(used(&store, &project, SUBNETS), 1);
        subnet(&client, &project, &n, "c", 3).unwrap();

        let open = tenant(&store, "open", SUBNETS, None);
        let n = net(&client, &open, "app").unwrap().meta.id;
        for i in 0..5 {
            subnet(&client, &open, &n, &format!("s{i}"), i).unwrap();
        }
        assert_eq!(used(&store, &open, SUBNETS), 5);
    }

    #[test]
    fn routers_are_limited_and_a_delete_frees_one() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", ROUTERS, Some(1));
        let first = router(&client, &project, "a").unwrap();
        assert_eq!(used(&store, &project, ROUTERS), 1);
        let err = router(&client, &project, "b").unwrap_err();
        refused(&err, ROUTERS, 1, 1);
        assert_eq!(client.routers().list(Some(&project)).unwrap().len(), 1);

        client.delete_router(&first.meta.id).unwrap();
        assert_eq!(used(&store, &project, ROUTERS), 0);
        router(&client, &project, "b").unwrap();

        let open = tenant(&store, "open", ROUTERS, None);
        for i in 0..4 {
            router(&client, &open, &format!("r{i}")).unwrap();
        }
        assert_eq!(used(&store, &open, ROUTERS), 4);
    }

    #[test]
    fn a_router_that_fails_to_plumb_does_not_stay_counted() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", ROUTERS, Some(1));
        // An external network with no subnet cannot take a gateway port.
        let bare = client
            .create_network(
                Meta::new(kind::NETWORK, "public", Some(project.clone())),
                NetworkSpec {
                    external: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let spec = RouterSpec {
            external_network: Some(bare.meta.id),
            interfaces: vec![],
            routes: vec![],
        };
        let meta = Meta::new(kind::ROUTER, "edge", Some(project.clone()));
        assert!(client.create_router(meta, spec).is_err());
        assert_eq!(used(&store, &project, ROUTERS), 0);
        router(&client, &project, "edge").unwrap();
    }

    #[test]
    fn security_groups_are_limited_and_the_default_one_is_free() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", SECURITY_GROUPS, Some(1));
        // A port brings the project's default group with it.
        let (n, _) = network(&client, &project, "app");
        create_port(&client, &project, &n, "web").unwrap();
        assert_eq!(
            client.security_groups().list(Some(&project)).unwrap().len(),
            1
        );

        let first = group(&client, &project, "web").unwrap();
        assert_eq!(used(&store, &project, SECURITY_GROUPS), 1);
        let err = group(&client, &project, "db").unwrap_err();
        refused(&err, SECURITY_GROUPS, 1, 1);
        assert_eq!(
            client.security_groups().list(Some(&project)).unwrap().len(),
            2
        );

        client.delete_security_group(&first.meta.id).unwrap();
        assert_eq!(used(&store, &project, SECURITY_GROUPS), 0);
        group(&client, &project, "db").unwrap();

        let open = tenant(&store, "open", SECURITY_GROUPS, None);
        for i in 0..5 {
            group(&client, &open, &format!("g{i}")).unwrap();
        }
        assert_eq!(used(&store, &open, SECURITY_GROUPS), 5);
    }

    #[test]
    fn concurrent_creates_stop_at_the_limit() {
        let (store, client) = harness();
        let project = tenant(&store, "tight", NETWORKS, Some(3));
        let threads: Vec<_> = (0..12)
            .map(|i| {
                let (client, project) = (client.clone(), project.clone());
                std::thread::spawn(move || net(&client, &project, &format!("n{i}")).is_ok())
            })
            .collect();
        let made = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(made, 3);
        assert_eq!(client.networks().list(Some(&project)).unwrap().len(), 3);
        assert_eq!(used(&store, &project, NETWORKS), 3);
    }
}
