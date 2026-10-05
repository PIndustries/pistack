// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Read the IPv4 address forge recorded for a machine, and keep one
//! automatic floating IP pointed at it.
//!
//! fabric does not depend on forge. This view ignores every other field.
//! Floating IPs this pass creates are labelled [`AUTO_FIP_LABEL`]; anything
//! without that label, including a floating IP someone associated by hand,
//! is never deleted. Association does not require a router: host NAT has
//! none, and the mock backend's router check happens later, in reconcile.

use crate::types::{FloatingIp, FloatingIpSpec, Network};
use crate::Client;
use pistack_store::{Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;

/// Label on floating IPs this pass owns. Value is `true`.
pub const AUTO_FIP_LABEL: &str = "pistack.io/auto-fip";
/// The machine id a labelled floating IP was created for.
const MACHINE_LABEL: &str = "pistack.io/machine";

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct MachineSpecView {}

impl Spec for MachineSpecView {
    const KIND: &'static str = kind::MACHINE;
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct MachineStatusView {
    #[serde(default)]
    addresses: Vec<String>,
}

/// A machine and the first IPv4 forge reported for it.
pub(crate) struct GuestAddress {
    pub meta: Meta,
    pub ipv4: IpAddr,
}

/// First IPv4 in `status.addresses`. A trailing prefix (`10.88.0.187/24`) is stripped.
pub(crate) fn guest_ipv4(store: &Store, machine: &Id) -> ApiResult<GuestAddress> {
    if machine.kind != kind::MACHINE {
        return Err(ApiError::bad_request(format!("{machine} is not a machine")));
    }
    let placed = store
        .collection::<Resource<MachineSpecView, MachineStatusView>>()
        .get(machine)?;
    let ipv4 = first_ipv4(&placed.status.addresses)
        .ok_or_else(|| ApiError::unprocessable(format!("machine {machine} has no ipv4 address")))?;
    Ok(GuestAddress {
        meta: placed.meta,
        ipv4,
    })
}

struct ListedMachine {
    meta: Meta,
    ipv4: Option<IpAddr>,
}

fn first_ipv4(addresses: &[String]) -> Option<IpAddr> {
    addresses.iter().find_map(|raw| {
        let bare = raw.split('/').next()?.trim();
        let ip: IpAddr = bare.parse().ok()?;
        ip.is_ipv4().then_some(ip)
    })
}

fn list_machines(store: &Store) -> ApiResult<Vec<ListedMachine>> {
    let rows = store
        .collection::<Resource<MachineSpecView, MachineStatusView>>()
        .list(None)?;
    Ok(rows
        .into_iter()
        .map(|placed| ListedMachine {
            meta: placed.meta,
            ipv4: first_ipv4(&placed.status.addresses),
        })
        .collect())
}

fn is_auto(fip: &FloatingIp) -> bool {
    fip.meta.labels.get(AUTO_FIP_LABEL).map(String::as_str) == Some("true")
}

fn labeled_machine(fip: &FloatingIp) -> Option<Id> {
    fip.meta
        .labels
        .get(MACHINE_LABEL)
        .and_then(|s| Id::parse(kind::MACHINE, s))
}

/// A floating IP already serves this machine when it points at a port whose
/// device is the machine, or whose fixed IP is the guest address.
fn port_covers(fip: &FloatingIp, ports: &[crate::types::Port], machine: &Id, addr: IpAddr) -> bool {
    let Some(pid) = &fip.spec.port else {
        return false;
    };
    let Some(port) = ports.iter().find(|p| &p.meta.id == pid) else {
        return false;
    };
    port.spec.device.as_ref() == Some(machine) || port.spec.fixed_ips.contains(&addr)
}

impl Client {
    /// One floating IP per machine that has an IPv4, and none left behind
    /// for a machine that is gone.
    ///
    /// Idempotent. Picks the external network named `fleet` when one exists,
    /// otherwise the only external network. The floating IP is created in
    /// the machine's project, named after the machine (`{name}-fip` when
    /// that name is taken), then [`Client::associate_to_machine`]. A failed
    /// associate deletes the floating IP just created so the pool does not
    /// leak; the next pass retries. Unlabeled floating IPs are left alone.
    pub fn sync_auto_floating_ips(&self) -> ApiResult<()> {
        let machines = list_machines(self.store())?;
        let live: HashSet<Id> = machines.iter().map(|m| m.meta.id.clone()).collect();
        let network = self.auto_fip_network()?;
        let fips = self.floating_ips().list(None)?;
        let ports = self.ports().list(None)?;
        let mut noted_no_network = false;

        for machine in &machines {
            let Some(addr) = machine.ipv4 else {
                continue;
            };
            if fips
                .iter()
                .any(|f| port_covers(f, &ports, &machine.meta.id, addr))
            {
                continue;
            }
            if !self.address_has_a_home(&machine.meta, addr, &ports) {
                // Associate would fail with "no subnet contains" every pass,
                // and a new floating IP would be created and rolled back each
                // time. Wait until a subnet covers the address.
                tracing::debug!(
                    machine = %machine.meta.id,
                    address = %addr,
                    "fabric auto-fip: no subnet contains the guest address; skipped"
                );
                continue;
            }
            if let Some(existing) = fips
                .iter()
                .find(|f| is_auto(f) && labeled_machine(f).as_ref() == Some(&machine.meta.id))
            {
                // Created earlier, not pointing at the guest yet. Retry
                // rather than allocating a second address.
                if let Err(e) = self.associate_to_machine(&existing.meta.id, &machine.meta.id) {
                    tracing::warn!(
                        machine = %machine.meta.id,
                        floating_ip = %existing.meta.id,
                        error = %e,
                        "fabric auto-fip: associate failed"
                    );
                }
                continue;
            }
            let Some(net) = &network else {
                if !noted_no_network {
                    tracing::debug!("fabric auto-fip: no external network to allocate from");
                    noted_no_network = true;
                }
                continue;
            };
            let Some(name) = self.auto_fip_name(&machine.meta) else {
                continue;
            };
            let mut meta = Meta::new(kind::FLOATING_IP, name, machine.meta.project.clone());
            meta.labels
                .insert(AUTO_FIP_LABEL.to_owned(), "true".to_owned());
            meta.labels
                .insert(MACHINE_LABEL.to_owned(), machine.meta.id.to_string());
            let created = match self.create_floating_ip(
                meta,
                FloatingIpSpec {
                    network: net.meta.id.clone(),
                    address: None,
                    port: None,
                },
            ) {
                Ok(fip) => fip,
                Err(e) => {
                    tracing::warn!(
                        machine = %machine.meta.id,
                        error = %e,
                        "fabric auto-fip: create failed"
                    );
                    continue;
                }
            };
            if let Err(e) = self.associate_to_machine(&created.meta.id, &machine.meta.id) {
                tracing::warn!(
                    machine = %machine.meta.id,
                    floating_ip = %created.meta.id,
                    error = %e,
                    "fabric auto-fip: associate failed"
                );
                if let Err(e) = self.delete_floating_ip(&created.meta.id) {
                    tracing::warn!(
                        floating_ip = %created.meta.id,
                        error = %e,
                        "fabric auto-fip: rollback failed"
                    );
                }
            }
        }
        self.reap_auto_floating_ips(&live)
    }

    /// True when associate can find or make a port for `addr`: a port in the
    /// machine's project already holds it, or a subnet the machine may use
    /// contains it. A store error counts as true so associate reports it.
    fn address_has_a_home(
        &self,
        machine: &Meta,
        addr: IpAddr,
        ports: &[crate::types::Port],
    ) -> bool {
        if ports
            .iter()
            .any(|p| p.meta.project == machine.project && p.spec.fixed_ips.contains(&addr))
        {
            return true;
        }
        self.subnet_containing(machine, addr)
            .map(|s| s.is_some())
            .unwrap_or(true)
    }

    /// `fleet` if that external network exists, else the sole external
    /// network. Several external networks and no `fleet` is ambiguous.
    fn auto_fip_network(&self) -> ApiResult<Option<Network>> {
        let mut external: Vec<Network> = self
            .networks()
            .list(None)?
            .into_iter()
            .filter(|n| n.spec.external)
            .collect();
        external.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
        if let Some(fleet) = external.iter().find(|n| n.meta.name == "fleet").cloned() {
            return Ok(Some(fleet));
        }
        if external.len() == 1 {
            return Ok(external.pop());
        }
        Ok(None)
    }

    fn auto_fip_name(&self, machine: &Meta) -> Option<String> {
        let project = machine.project.as_ref();
        if self.fip_name_free(project, &machine.name) {
            return Some(machine.name.clone());
        }
        let alt = format!("{}-fip", machine.name);
        if self.fip_name_free(project, &alt) {
            return Some(alt);
        }
        tracing::warn!(
            machine = %machine.id,
            name = %machine.name,
            "fabric auto-fip: name taken"
        );
        None
    }

    fn fip_name_free(&self, project: Option<&Id>, name: &str) -> bool {
        matches!(
            self.floating_ips().get_by_name(project, name),
            Err(StoreError::NotFound(_))
        )
    }

    fn machine_of_auto_fip(&self, fip: &FloatingIp) -> Option<Id> {
        if let Some(id) = labeled_machine(fip) {
            return Some(id);
        }
        let pid = fip.spec.port.as_ref()?;
        let port = self.ports().get(pid).ok()?;
        port.spec.device.filter(|d| d.kind == kind::MACHINE)
    }

    /// Drop labelled floating IPs whose machine is gone, and the port they
    /// pointed at. A port whose device is a machine that still exists stays,
    /// as does a port some other floating IP still uses.
    fn reap_auto_floating_ips(&self, live: &HashSet<Id>) -> ApiResult<()> {
        let fips = self.floating_ips().list(None)?;
        for fip in &fips {
            if !is_auto(fip) {
                continue;
            }
            let Some(machine) = self.machine_of_auto_fip(fip) else {
                continue;
            };
            if live.contains(&machine) {
                continue;
            }
            let port_id = fip.spec.port.clone();
            if let Err(e) = self.delete_floating_ip(&fip.meta.id) {
                tracing::warn!(
                    floating_ip = %fip.meta.id,
                    error = %e,
                    "fabric auto-fip: delete failed"
                );
                continue;
            }
            let Some(pid) = port_id else {
                continue;
            };
            let delete_port = match self.ports().get(&pid) {
                Ok(port) => !port.spec.device.as_ref().is_some_and(|d| live.contains(d)),
                Err(StoreError::NotFound(_)) => false,
                Err(e) => {
                    tracing::warn!(port = %pid, error = %e, "fabric auto-fip: port lookup failed");
                    continue;
                }
            };
            if !delete_port {
                continue;
            }
            let still_used = self
                .floating_ips()
                .list(None)?
                .iter()
                .any(|other| other.spec.port.as_ref() == Some(&pid));
            if still_used {
                continue;
            }
            if let Err(e) = self.delete_port(&pid) {
                tracing::warn!(port = %pid, error = %e, "fabric auto-fip: port delete failed");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use crate::Client;
    use crate::MockBackend;
    use pistack_types::error::ErrorCode;
    use std::sync::Arc;

    fn client() -> (Store, Client, Id) {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
        client.ensure_default_template().unwrap();
        let project = Id::new(kind::PROJECT);
        (store, client, project)
    }

    fn put_machine(store: &Store, name: &str, project: &Id, addresses: Vec<String>) -> Id {
        let mut machine = Resource::<MachineSpecView, MachineStatusView>::new(
            name,
            Some(project.clone()),
            MachineSpecView {},
        );
        machine.status.addresses = addresses;
        store
            .collection::<Resource<MachineSpecView, MachineStatusView>>()
            .create(machine)
            .unwrap()
            .meta
            .id
    }

    fn subnet(client: &Client, project: &Id, net: &Id, name: &str, cidr: &str, gw: &str) {
        client
            .create_subnet(
                Meta::new(kind::SUBNET, name, Some(project.clone())),
                SubnetSpec {
                    network: net.clone(),
                    cidr: cidr.parse().unwrap(),
                    gateway: Some(gw.parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap();
    }

    #[test]
    fn associate_to_machine_claims_the_guest_address() {
        let (store, client, project) = client();
        let guest_net = client
            .create_network(
                Meta::new(kind::NETWORK, "guest", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        subnet(
            &client,
            &project,
            &guest_net.meta.id,
            "guest-net",
            "10.88.0.0/24",
            "10.88.0.1",
        );
        let ext_spec = NetworkSpec {
            external: true,
            ..Default::default()
        };
        let ext = client
            .create_network(
                Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
                ext_spec,
            )
            .unwrap();
        subnet(
            &client,
            &project,
            &ext.meta.id,
            "fleet-pool",
            "10.66.66.80/28",
            "10.66.66.81",
        );
        let fip = client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "demo", Some(project.clone())),
                FloatingIpSpec {
                    network: ext.meta.id.clone(),
                    address: None,
                    port: None,
                },
            )
            .unwrap();
        assert_eq!(
            fip.spec.address,
            Some("10.66.66.82".parse().unwrap()),
            "gateway .81 is reserved, so the first floating address is .82"
        );
        let machine = put_machine(
            &store,
            "demo",
            &project,
            vec!["10.88.0.187/24".into(), "fe80::1".into()],
        );
        let bound = client.associate_to_machine(&fip.meta.id, &machine).unwrap();
        let port_id = bound.spec.port.clone().unwrap();
        let port = client.ports().get(&port_id).unwrap();
        assert_eq!(
            port.spec.fixed_ips,
            vec!["10.88.0.187".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(port.spec.device.as_ref(), Some(&machine));
        assert_eq!(port.spec.network, guest_net.meta.id);

        let again = client.associate_to_machine(&fip.meta.id, &machine).unwrap();
        assert_eq!(again.spec.port, Some(port_id));
        assert_eq!(client.ports().list(None).unwrap().len(), 1);
    }

    #[test]
    fn longest_prefix_subnet_wins() {
        let (store, client, project) = client();
        let wide = client
            .create_network(
                Meta::new(kind::NETWORK, "wide", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        let narrow = client
            .create_network(
                Meta::new(kind::NETWORK, "narrow", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        subnet(
            &client,
            &project,
            &wide.meta.id,
            "wide",
            "10.88.0.0/24",
            "10.88.0.1",
        );
        subnet(
            &client,
            &project,
            &narrow.meta.id,
            "narrow",
            "10.88.0.128/25",
            "10.88.0.129",
        );
        let ext_spec = NetworkSpec {
            external: true,
            ..Default::default()
        };
        let ext = client
            .create_network(
                Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
                ext_spec,
            )
            .unwrap();
        subnet(
            &client,
            &project,
            &ext.meta.id,
            "pool",
            "10.66.66.80/28",
            "10.66.66.81",
        );
        let fip = client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "demo", Some(project.clone())),
                FloatingIpSpec {
                    network: ext.meta.id,
                    address: None,
                    port: None,
                },
            )
            .unwrap();
        let machine = put_machine(&store, "demo", &project, vec!["10.88.0.187".into()]);
        let bound = client.associate_to_machine(&fip.meta.id, &machine).unwrap();
        let port = client
            .ports()
            .get(bound.spec.port.as_ref().unwrap())
            .unwrap();
        assert_eq!(port.spec.network, narrow.meta.id);
    }

    #[test]
    fn missing_subnet_and_missing_address_are_errors() {
        let (store, client, project) = client();
        let ext_spec = NetworkSpec {
            external: true,
            ..Default::default()
        };
        let ext = client
            .create_network(
                Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
                ext_spec,
            )
            .unwrap();
        subnet(
            &client,
            &project,
            &ext.meta.id,
            "pool",
            "10.66.66.80/28",
            "10.66.66.81",
        );
        let fip = client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "demo", Some(project.clone())),
                FloatingIpSpec {
                    network: ext.meta.id,
                    address: None,
                    port: None,
                },
            )
            .unwrap();
        let machine = put_machine(&store, "demo", &project, vec!["10.88.0.187".into()]);
        let err = client
            .associate_to_machine(&fip.meta.id, &machine)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Unprocessable);
        assert!(err.message.contains("no subnet contains 10.88.0.187"));

        let bare = put_machine(&store, "bare", &project, vec!["fe80::1".into()]);
        let err = client
            .associate_to_machine(&fip.meta.id, &bare)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Unprocessable);
        assert!(err.message.contains("no ipv4"));
    }

    fn external(client: &Client, project: &Id, name: &str, cidr: &str, gw: &str) -> Id {
        let spec = NetworkSpec {
            external: true,
            ..Default::default()
        };
        let net = client
            .create_network(Meta::new(kind::NETWORK, name, Some(project.clone())), spec)
            .unwrap();
        subnet(client, project, &net.meta.id, name, cidr, gw);
        net.meta.id
    }

    fn guest(client: &Client, project: &Id) {
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "guest", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        subnet(
            client,
            project,
            &net.meta.id,
            "guest-net",
            "10.88.0.0/24",
            "10.88.0.1",
        );
    }

    fn delete_machine(store: &Store, id: &Id) {
        store
            .collection::<Resource<MachineSpecView, MachineStatusView>>()
            .delete(id)
            .unwrap();
    }

    fn auto_for(client: &Client, machine: &Id) -> FloatingIp {
        let found: Vec<_> = client
            .floating_ips()
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|f| {
                f.meta.labels.get(super::AUTO_FIP_LABEL).map(String::as_str) == Some("true")
                    && f.spec.port.as_ref().is_some_and(|pid| {
                        client.ports().get(pid).unwrap().spec.device.as_ref() == Some(machine)
                    })
            })
            .collect();
        assert_eq!(found.len(), 1, "auto floating ips for {machine}");
        found.into_iter().next().unwrap()
    }

    #[test]
    fn auto_fip_tracks_machines() {
        let (store, client, project) = client();
        guest(&client, &project);
        let fleet = external(&client, &project, "fleet", "10.66.66.80/28", "10.66.66.81");
        let _other = external(&client, &project, "edge", "203.0.113.0/24", "203.0.113.1");

        let demo = put_machine(&store, "demo", &project, vec!["10.88.0.187/24".into()]);
        let manual = client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "demo", Some(project.clone())),
                FloatingIpSpec {
                    network: fleet.clone(),
                    address: None,
                    port: None,
                },
            )
            .unwrap();
        assert_eq!(manual.spec.address, Some("10.66.66.82".parse().unwrap()));
        let manual = client.associate_to_machine(&manual.meta.id, &demo).unwrap();
        let manual_port = manual.spec.port.clone().unwrap();

        let web = put_machine(
            &store,
            "web",
            &project,
            vec!["10.88.0.50/24".into(), "fe80::1".into()],
        );
        let quiet = put_machine(&store, "quiet", &project, vec!["fe80::1".into()]);
        let stay = put_machine(&store, "stay", &project, vec!["10.88.0.60".into()]);

        client.sync_auto_floating_ips().unwrap();

        let web_fip = auto_for(&client, &web);
        assert_eq!(web_fip.meta.name, "web");
        assert_eq!(web_fip.spec.network, fleet);
        assert_eq!(
            web_fip
                .meta
                .labels
                .get(super::AUTO_FIP_LABEL)
                .map(String::as_str),
            Some("true")
        );
        let web_port = client
            .ports()
            .get(web_fip.spec.port.as_ref().unwrap())
            .unwrap();
        assert_eq!(web_port.spec.device.as_ref(), Some(&web));
        assert_eq!(
            web_port.spec.fixed_ips,
            vec!["10.88.0.50".parse::<IpAddr>().unwrap()]
        );
        let stay_fip = auto_for(&client, &stay);
        assert_eq!(stay_fip.spec.network, fleet);
        assert!(client.ports_of_device(&quiet).unwrap().is_empty());
        assert!(client
            .floating_ips()
            .get_by_name(Some(&project), "quiet")
            .is_err());
        // demo already had a floating IP; the pass must not add one.
        let demo_fips: Vec<_> = client
            .floating_ips()
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|f| f.spec.port.as_ref() == Some(&manual_port))
            .collect();
        assert_eq!(demo_fips.len(), 1);
        assert_eq!(demo_fips[0].meta.id, manual.meta.id);
        assert!(!demo_fips[0].meta.labels.contains_key(super::AUTO_FIP_LABEL));
        assert_eq!(client.floating_ips().list(None).unwrap().len(), 3);

        let web_id = web_fip.meta.id.clone();
        let web_addr = web_fip.spec.address;
        let stay_id = stay_fip.meta.id.clone();
        let stay_port = stay_fip.spec.port.clone().unwrap();
        client.sync_auto_floating_ips().unwrap();
        assert_eq!(client.floating_ips().list(None).unwrap().len(), 3);
        let again = client.floating_ips().get(&web_id).unwrap();
        assert_eq!(again.spec.address, web_addr);
        assert_eq!(again.spec.port, web_fip.spec.port);

        delete_machine(&store, &web);
        delete_machine(&store, &demo);
        client.sync_auto_floating_ips().unwrap();

        assert!(client.floating_ips().try_get(&web_id).unwrap().is_none());
        assert!(client.ports().try_get(&web_port.meta.id).unwrap().is_none());
        let kept = client.floating_ips().get(&manual.meta.id).unwrap();
        assert!(!kept.meta.labels.contains_key(super::AUTO_FIP_LABEL));
        assert_eq!(kept.spec.port.as_ref(), Some(&manual_port));
        assert!(client.ports().get(&manual_port).is_ok());
        assert!(client.floating_ips().get(&stay_id).is_ok());
        assert!(client.ports().get(&stay_port).is_ok());
        let _ = quiet;
    }

    #[test]
    fn auto_fip_uses_only_external_and_fallback_name() {
        let (store, client, project) = client();
        guest(&client, &project);
        let edge = external(&client, &project, "edge", "10.66.66.80/28", "10.66.66.81");
        client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "box", Some(project.clone())),
                FloatingIpSpec {
                    network: edge.clone(),
                    address: None,
                    port: None,
                },
            )
            .unwrap();
        let machine = put_machine(&store, "box", &project, vec!["10.88.0.9".into()]);
        client.sync_auto_floating_ips().unwrap();
        let fip = auto_for(&client, &machine);
        assert_eq!(fip.meta.name, "box-fip");
        assert_eq!(fip.spec.network, edge);
        client.sync_auto_floating_ips().unwrap();
        assert_eq!(client.floating_ips().list(Some(&project)).unwrap().len(), 2);
        delete_machine(&store, &machine);
        client.sync_auto_floating_ips().unwrap();
        let left = client.floating_ips().list(Some(&project)).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].meta.name, "box");
        assert!(!left[0].meta.labels.contains_key(super::AUTO_FIP_LABEL));
    }

    #[test]
    fn auto_fip_associate_failure_does_not_leak() {
        let (store, client, project) = client();
        let _fleet = external(&client, &project, "fleet", "10.66.66.80/28", "10.66.66.81");
        let _machine = put_machine(&store, "web", &project, vec!["10.88.0.50".into()]);
        client.sync_auto_floating_ips().unwrap();
        assert!(client.floating_ips().list(None).unwrap().is_empty());
        assert!(client.ports().list(None).unwrap().is_empty());
    }

    #[test]
    fn auto_fip_skips_address_outside_every_subnet() {
        let (store, client, project) = client();
        let _fleet = external(&client, &project, "fleet", "10.66.66.80/28", "10.66.66.81");
        guest(&client, &project);
        let _machine = put_machine(&store, "lost", &project, vec!["10.89.0.180".into()]);
        let mut events = store.events().subscribe();
        for _ in 0..3 {
            client.sync_auto_floating_ips().unwrap();
            assert!(client.floating_ips().list(None).unwrap().is_empty());
            assert!(client.ports().list(None).unwrap().is_empty());
        }
        // Not even a create-then-rollback: no floating IP was written at all.
        while let Ok(ev) = events.try_recv() {
            assert_ne!(ev.kind, kind::FLOATING_IP, "{ev:?}");
        }
        let found = put_machine(&store, "found", &project, vec!["10.88.0.60".into()]);
        client.sync_auto_floating_ips().unwrap();
        let fip = auto_for(&client, &found);
        assert!(fip.spec.port.is_some());
        assert_eq!(client.floating_ips().list(None).unwrap().len(), 1);
    }

    #[test]
    fn auto_fip_skips_when_no_external_network() {
        let (store, client, project) = client();
        guest(&client, &project);
        let _machine = put_machine(&store, "web", &project, vec!["10.88.0.50".into()]);
        client.sync_auto_floating_ips().unwrap();
        assert!(client.floating_ips().list(None).unwrap().is_empty());
    }
}
