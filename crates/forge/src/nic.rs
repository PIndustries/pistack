// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Allocate a fabric port before a guest boots, and describe that port to
//! the host. An empty `spec.networks` leaves the shared bridge alone.

use fabric::{Client, HostNat, MockBackend, NetBackend, Port, Router, Subnet};
use pistack_rpc::proto::NicAttachment;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use std::net::Ipv4Addr;
use std::sync::Arc;

use crate::types::Machine;

/// True when the machine named at least one network.
pub fn wants_port(m: &Machine) -> bool {
    !named(&m.spec.networks).is_empty()
}

/// Create the missing port for each named network. A second call reuses the
/// port. No names means no port, which is how `demo` and `web` stay put.
pub async fn ensure_ports(store: &Store, m: &Machine) -> ApiResult<()> {
    let names = named(&m.spec.networks);
    if names.is_empty() {
        return Ok(());
    }
    let Some(project) = m.meta.project.clone() else {
        return Err(ApiError::bad_request(
            "a project network needs a project on the machine",
        ));
    };
    let client = fabric_client(store);
    let groups = resolve_groups(&client, &project, &m.spec.security_groups)?;
    for name in names {
        let net = resolve_network(&client, &project, &name)?;
        let exists = client
            .ports_of_device(&m.meta.id)?
            .into_iter()
            .any(|p| p.spec.network == net.meta.id);
        if exists {
            continue;
        }
        client
            .create_port_for_device_with(&project, &name, m.meta.id.clone(), &groups)
            .await?;
    }
    Ok(())
}

/// The NICs the host should boot. Fails when a named network has no port.
pub fn attachments(store: &Store, m: &Machine) -> ApiResult<Vec<NicAttachment>> {
    let names = named(&m.spec.networks);
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let Some(project) = m.meta.project.as_ref() else {
        return Err(ApiError::bad_request(
            "a project network needs a project on the machine",
        ));
    };
    let client = fabric_client(store);
    let ports = client.ports_of_device(&m.meta.id)?;
    let mut nics = Vec::with_capacity(names.len());
    for name in names {
        let net = resolve_network(&client, project, &name)?;
        let port = ports
            .iter()
            .find(|p| p.spec.network == net.meta.id)
            .ok_or_else(|| {
                ApiError::unavailable(format!("machine {} has no port on {name}", m.meta.id))
            })?;
        nics.push(nic_from(&client, port)?);
    }
    Ok(nics)
}

/// Drop ports plugged into `device`. No port is success.
pub fn release_ports(store: &Store, device: &Id) -> ApiResult<()> {
    fabric_client(store)
        .release_device_ports(device)
        .map(|_| ())
}

fn named(names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for n in names {
        let n = n.trim();
        if n.is_empty() || out.iter().any(|seen| seen == n) {
            continue;
        }
        out.push(n.to_owned());
    }
    out
}

fn fabric_client(store: &Store) -> Client {
    let backend: Arc<dyn NetBackend> = match std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref()
    {
        Some("hostnat") => Arc::new(HostNat::new()),
        Some("overlay") => Arc::new(fabric::OverlayBackend::new()),
        _ => Arc::new(MockBackend::new()),
    };
    Client::new(store.clone(), backend)
}

fn resolve_network(client: &Client, project: &Id, name: &str) -> ApiResult<fabric::Network> {
    match client.networks().resolve(Some(project), name) {
        Ok(n) => Ok(n),
        Err(pistack_store::StoreError::NotFound(_)) => {
            client.networks().resolve(None, name).map_err(Into::into)
        }
        Err(e) => Err(e.into()),
    }
}

fn resolve_groups(client: &Client, project: &Id, names: &[String]) -> ApiResult<Vec<Id>> {
    let mut out = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let sg = client
            .security_groups()
            .resolve(Some(project), name)
            .map_err(|_| ApiError::unprocessable(format!("security group '{name}' not found")))?;
        out.push(sg.meta.id);
    }
    Ok(out)
}

fn nic_from(client: &Client, port: &Port) -> ApiResult<NicAttachment> {
    let address = port
        .spec
        .fixed_ips
        .iter()
        .find(|ip| ip.is_ipv4())
        .map(|ip| ip.to_string())
        .ok_or_else(|| {
            ApiError::bad_request(format!("port {} has no ipv4 address", port.meta.id))
        })?;
    let subnet_id = port
        .spec
        .subnet
        .clone()
        .ok_or_else(|| ApiError::bad_request(format!("port {} has no subnet", port.meta.id)))?;
    let subnet = client.subnets().get(&subnet_id)?;
    let net = client.networks().get(&port.spec.network)?;
    let (gateway, prefix) = gateway_and_prefix(&subnet)?;
    if port.spec.mac.is_empty() {
        return Err(ApiError::bad_request(format!(
            "port {} has no mac",
            port.meta.id
        )));
    }
    Ok(NicAttachment {
        network_id: port.spec.network.to_string(),
        mac: port.spec.mac.clone(),
        address: address.clone(),
        prefix,
        gateway,
        routed: is_routed(client, &subnet_id),
        segment_id: crate::overlay::segment_for_nic(&net, &address)?,
    })
}

fn is_routed(client: &Client, subnet: &Id) -> bool {
    client
        .routers()
        .list(None)
        .ok()
        .into_iter()
        .flatten()
        .any(|r: Router| {
            r.spec.external_network.is_some() && r.spec.interfaces.iter().any(|i| i == subnet)
        })
}

fn gateway_and_prefix(sub: &Subnet) -> ApiResult<(String, u8)> {
    let cidr = sub.spec.cidr.to_string();
    let (addr, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| ApiError::bad_request(format!("subnet {} has no prefix", sub.meta.id)))?;
    if addr.contains(':') {
        return Err(ApiError::bad_request(format!(
            "subnet {} is not ipv4",
            sub.meta.id
        )));
    }
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| ApiError::bad_request(format!("subnet {} has a bad prefix", sub.meta.id)))?;
    let gateway = match sub.spec.gateway {
        Some(ip) if ip.is_ipv4() => ip.to_string(),
        Some(_) => {
            return Err(ApiError::bad_request(format!(
                "subnet {} gateway is not ipv4",
                sub.meta.id
            )))
        }
        None => {
            let parsed: Ipv4Addr = addr.parse().map_err(|_| {
                ApiError::bad_request(format!("subnet {} is not ipv4", sub.meta.id))
            })?;
            Ipv4Addr::from(u32::from(parsed).saturating_add(1)).to_string()
        }
    };
    Ok((gateway, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{new_machine, MachineSpec};
    use fabric::{NetworkSpec, SubnetSpec};
    use pistack_types::{kind, Meta};

    fn spec_net(name: &str) -> MachineSpec {
        MachineSpec {
            size: "s.1x1".into(),
            image: "ubuntu-24.04".into(),
            networks: vec![name.into()],
            ..MachineSpec::default()
        }
    }

    #[tokio::test]
    async fn the_port_address_is_the_nic_and_a_router_marks_it_routed() {
        let store = Store::in_memory().unwrap();
        let client = fabric_client(&store);
        let project = Id::new(kind::PROJECT);
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "app", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        let sub = client
            .create_subnet(
                Meta::new(kind::SUBNET, "app-net", Some(project.clone())),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: "10.90.0.0/24".parse().unwrap(),
                    gateway: Some("10.90.0.1".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap();
        let machine = new_machine("box", Some(project.clone()), spec_net("app"));
        ensure_ports(&store, &machine).await.unwrap();
        ensure_ports(&store, &machine).await.unwrap();
        assert_eq!(client.ports_of_device(&machine.meta.id).unwrap().len(), 1);

        let nics = attachments(&store, &machine).unwrap();
        assert_eq!(nics.len(), 1);
        assert_eq!(nics[0].address, "10.90.0.2");
        assert_ne!(nics[0].address, "10.90.0.1");
        assert_eq!(nics[0].prefix, 24);
        assert_eq!(nics[0].gateway, "10.90.0.1");
        assert!(!nics[0].routed);
        assert!(nics[0].mac.contains(':'));
        assert_eq!(nics[0].network_id, net.meta.id.to_string());

        let mut ext = NetworkSpec::default();
        ext.external = true;
        let fleet = client
            .create_network(
                Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
                ext,
            )
            .unwrap();
        client
            .create_subnet(
                Meta::new(kind::SUBNET, "fleet-pool", Some(project.clone())),
                SubnetSpec {
                    network: fleet.meta.id.clone(),
                    cidr: "10.66.66.80/28".parse().unwrap(),
                    gateway: Some("10.66.66.81".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: false,
                    host_routes: vec![],
                },
            )
            .unwrap();
        client
            .create_router(
                Meta::new(kind::ROUTER, "edge", Some(project)),
                fabric::RouterSpec {
                    external_network: Some(fleet.meta.id),
                    interfaces: vec![sub.meta.id],
                    routes: vec![],
                },
            )
            .unwrap();
        let nics = attachments(&store, &machine).unwrap();
        assert!(nics[0].routed);

        let quiet = new_machine("demo", None, MachineSpec::default());
        ensure_ports(&store, &quiet).await.unwrap();
        assert!(attachments(&store, &quiet).unwrap().is_empty());
    }
}
