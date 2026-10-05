// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Build the per-host overlay plan from fabric records.
//!
//! One plan per host that has a guest on the segment. Every such plan lists
//! every bound port, so the forwarding table reaches the host that owns the
//! tap. The project router — gateway and outbound translation — is realised
//! on exactly one of those hosts, the lowest underlay address. A floating
//! address on an overlay segment or a provider network is installed on the
//! host that owns the guest port. On any other external network it stays on
//! the project router. The shared 10.88.0.0/24 network is left on host NAT.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};

use fabric::overlay::{
    is_private_ula, plan, DhcpLease, DhcpRoute, DnatRule, OverlayPlan, PlanFloat, PlanHost,
    PlanNetwork, PlanPort, PlanProvider, PlanRouter, PortAddress, SegmentPlan, SgRule,
    PLAN_VERSION,
};
use fabric::{
    Direction, EtherType, FloatingIp, Network, NetworkKind, Port, Protocol, Remote, Router,
    SecurityGroup, Subnet,
};
use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id};
use serde_json::Value;

use crate::types::{usable_underlay, Host, Machine};

/// True when this process should realise tenant networks as overlays.
pub fn enabled() -> bool {
    std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref() == Some("overlay")
}

/// VNI to put on a NIC, or `None` when this NIC stays on host NAT.
///
/// An overlay network with no segment yet is `unavailable`, so the caller
/// can retry after fabric assigns one. Addresses in 10.88.0.0/24 stay on
/// the shared bridge even when the overlay backend is selected.
pub fn segment_for_nic(net: &Network, address: &str) -> ApiResult<Option<u32>> {
    if !enabled()
        || shared_nat(address)
        || !matches!(net.spec.kind, NetworkKind::Overlay)
        || net.status.datapath.as_deref() == Some("hostnat")
    {
        return Ok(None);
    }
    match net.status.segment_id {
        Some(vni) if (1..=16_777_215).contains(&vni) => Ok(Some(vni)),
        _ => Err(ApiError::unavailable(format!(
            "overlay network {} has no segment",
            net.meta.id
        ))),
    }
}

/// Plans for every host that holds a guest, a project router, or a floating
/// binding on an overlay segment or a provider network.
pub fn all_plans(store: &Store) -> ApiResult<Vec<(String, Value)>> {
    let hosts = store.collection::<Host>().list(None).map_err(store_err)?;
    let machines = store
        .collection::<Machine>()
        .list(None)
        .map_err(store_err)?;
    let networks = store
        .collection::<Network>()
        .list(None)
        .map_err(store_err)?;
    let subnets = store.collection::<Subnet>().list(None).map_err(store_err)?;
    let ports = store.collection::<Port>().list(None).map_err(store_err)?;
    let routers = store.collection::<Router>().list(None).map_err(store_err)?;
    let groups = store
        .collection::<SecurityGroup>()
        .list(None)
        .map_err(store_err)?;
    let fips = store
        .collection::<FloatingIp>()
        .list(None)
        .map_err(store_err)?;
    let guests = placed_guests(&ports, &machines, &hosts)?;

    let mut by_node: BTreeMap<String, (String, Vec<SegmentPlan>)> = BTreeMap::new();
    for net in &networks {
        let Some(kind) = classify(net, &subnets) else {
            continue;
        };
        let seeing = segment_hosts(net, &kind, &guests, &fips, &routers, &subnets);
        if seeing.is_empty() {
            continue;
        }
        if let Realised::Provider(provider) = &kind {
            if refused_physnet(&provider.physnet) {
                return Err(ApiError::unprocessable(format!(
                    "refusing physnet {}",
                    provider.physnet
                )));
            }
        }
        let on_net: Vec<&Guest> = guests
            .iter()
            .filter(|g| g.port.spec.network == net.meta.id)
            .collect();
        let designated = on_net.iter().map(|g| g.underlay).min();
        let plan_ports = on_net
            .iter()
            .map(|g| port_endpoint(g, &groups, &ports, &subnets))
            .collect::<ApiResult<Vec<_>>>()?;
        let routers_here = match designated {
            Some(_) => routers_on(
                &net.meta.id,
                &networks,
                &subnets,
                &routers,
                &ports,
                &fips,
                &on_net,
            )?,
            None => Vec::new(),
        };
        let (vni, provider) = match &kind {
            Realised::Tenant(vni) | Realised::External(vni) => (*vni, None),
            // A provider segment is not a VXLAN. The VNI stays 0.
            Realised::Provider(provider) => (0, Some(provider.clone())),
        };
        let force_metadata = matches!(kind, Realised::Tenant(_));
        for member in &seeing {
            let floating = floats_for(net, member.underlay, &guests, &fips, &routers, &subnets);
            let routers = if designated == Some(member.underlay) {
                routers_here.clone()
            } else {
                Vec::new()
            };
            // The fixed address has to be a port on the segment that hosts
            // the floating binding. A guest port lives on its tenant network,
            // so the external segment lists it too.
            let mut ports_here = plan_ports.clone();
            attach_float_ports(
                &mut ports_here,
                &floating,
                &guests,
                &groups,
                &ports,
                &subnets,
            )?;
            let segment = realise(
                member.underlay,
                PlanNetwork {
                    id: net.meta.id.to_string(),
                    vni,
                    mtu: net.spec.mtu,
                    ports: ports_here,
                    routers,
                    provider: provider.clone(),
                    floating,
                },
                force_metadata,
            )?;
            by_node
                .entry(member.node.clone())
                .or_insert_with(|| (member.underlay.to_string(), Vec::new()))
                .1
                .push(segment);
        }
    }

    let mut out = Vec::with_capacity(by_node.len());
    for (node, (underlay, networks)) in by_node {
        let doc = OverlayPlan {
            version: PLAN_VERSION,
            host: underlay,
            networks,
        };
        let value = serde_json::to_value(&doc)
            .map_err(|e| ApiError::internal(format!("overlay plan: {e}")))?;
        out.push((node, value));
    }
    Ok(out)
}

/// Plans for the networks named on `machine`, plus an external segment that
/// carries a floating binding or the router for one of those networks.
pub fn plans_for_machine(store: &Store, machine: &Machine) -> ApiResult<Vec<(String, Value)>> {
    let names: BTreeSet<String> = machine
        .spec
        .networks
        .iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let networks = store
        .collection::<Network>()
        .list(None)
        .map_err(store_err)?;
    let tenant: BTreeSet<String> = networks
        .iter()
        .filter(|n| names.contains(&n.meta.name) || names.contains(&n.meta.id.to_string()))
        .map(|n| n.meta.id.to_string())
        .collect();
    if tenant.is_empty() {
        return Ok(Vec::new());
    }
    let subnets = store.collection::<Subnet>().list(None).map_err(store_err)?;
    let routers = store.collection::<Router>().list(None).map_err(store_err)?;
    let ports = store.collection::<Port>().list(None).map_err(store_err)?;
    let fips = store
        .collection::<FloatingIp>()
        .list(None)
        .map_err(store_err)?;
    let mut keep = tenant.clone();
    for router in &routers {
        let attached = router.spec.interfaces.iter().any(|sid| {
            subnets
                .iter()
                .any(|s| &s.meta.id == sid && tenant.contains(&s.spec.network.to_string()))
        });
        if attached {
            if let Some(ext) = &router.spec.external_network {
                keep.insert(ext.to_string());
            }
        }
    }
    let machine_ports: BTreeSet<&Id> = ports
        .iter()
        .filter(|p| p.spec.device.as_ref() == Some(&machine.meta.id))
        .map(|p| &p.meta.id)
        .collect();
    for fip in &fips {
        if fip
            .spec
            .port
            .as_ref()
            .is_some_and(|p| machine_ports.contains(p))
        {
            keep.insert(fip.spec.network.to_string());
        }
    }
    let mut out = Vec::new();
    for (node, value) in all_plans(store)? {
        let mut plan: OverlayPlan = serde_json::from_value(value)
            .map_err(|e| ApiError::internal(format!("overlay plan: {e}")))?;
        plan.networks.retain(|n| keep.contains(&n.id));
        if plan.networks.is_empty() {
            continue;
        }
        let value = serde_json::to_value(&plan)
            .map_err(|e| ApiError::internal(format!("overlay plan: {e}")))?;
        out.push((node, value));
    }
    Ok(out)
}

struct Guest {
    port: Port,
    underlay: Ipv4Addr,
    node: String,
}

struct SeenHost {
    underlay: Ipv4Addr,
    node: String,
}

enum Realised {
    Tenant(u32),
    External(u32),
    Provider(PlanProvider),
}

fn classify(net: &Network, subnets: &[Subnet]) -> Option<Realised> {
    match &net.spec.kind {
        NetworkKind::Provider { physnet, vlan } => Some(Realised::Provider(PlanProvider {
            physnet: physnet.clone(),
            vlan: *vlan,
        })),
        NetworkKind::Overlay => {
            if shared_nat_network(net, subnets) {
                return None;
            }
            let vni = real_vni(net)?;
            if net.spec.external {
                Some(Realised::External(vni))
            } else {
                Some(Realised::Tenant(vni))
            }
        }
    }
}

fn real_vni(net: &Network) -> Option<u32> {
    match net.status.segment_id {
        Some(vni) if (1..=16_777_215).contains(&vni) => Some(vni),
        _ => None,
    }
}

/// Floating translation follows the guest port when the external network is
/// itself an overlay segment or a provider network. Anything else, including
/// 10.88.0.0/24, stays as router DNAT.
fn follows_port(net: &Network, subnets: &[Subnet]) -> bool {
    if shared_nat_network(net, subnets) {
        return false;
    }
    match &net.spec.kind {
        NetworkKind::Provider { .. } => true,
        NetworkKind::Overlay => real_vni(net).is_some(),
    }
}

fn shared_nat_network(net: &Network, subnets: &[Subnet]) -> bool {
    matches!(net.spec.kind, NetworkKind::Overlay)
        && subnets
            .iter()
            .any(|s| s.spec.network == net.meta.id && s.spec.cidr.to_string() == "10.88.0.0/24")
}

fn refused_physnet(name: &str) -> bool {
    name.is_empty()
        || name == "wg-fleet"
        || name == "pistack0"
        || name == "simbr0"
        || name.starts_with("pi")
}

/// IPv4 external addresses stay as they are. A unique-local IPv6 address is
/// SNAT for an IPv6-only router. A public IPv6 address is not an underlay
/// and is not an external address.
fn external_for(router: &Router, subnet: &Subnet, subnets: &[Subnet]) -> Option<String> {
    match router.status.external_ip? {
        IpAddr::V4(ip) => Some(ip.to_string()),
        IpAddr::V6(ip) => {
            if !is_private_ula(ip) || !subnet.spec.cidr.addr().is_ipv6() {
                return None;
            }
            let mixed = subnets.iter().any(|other| {
                router.spec.interfaces.iter().any(|id| id == &other.meta.id)
                    && other.spec.cidr.addr().is_ipv4()
            });
            if mixed {
                None
            } else {
                Some(ip.to_string())
            }
        }
    }
}

fn placed_guests(ports: &[Port], machines: &[Machine], hosts: &[Host]) -> ApiResult<Vec<Guest>> {
    let mut out = Vec::new();
    let mut underlay_owner: BTreeMap<Ipv4Addr, String> = BTreeMap::new();
    for port in ports {
        if port.is_router_port() {
            continue;
        }
        let Some(device) = port.spec.device.as_ref() else {
            continue;
        };
        if device.kind != kind::MACHINE {
            continue;
        }
        let Some(machine) = machines.iter().find(|m| &m.meta.id == device) else {
            continue;
        };
        let Some(host_id) = machine.status.host.as_ref() else {
            continue;
        };
        let Some(host) = hosts.iter().find(|h| &h.meta.id == host_id) else {
            continue;
        };
        let Some(raw) = usable_underlay(host.spec.underlay_ip.as_deref()) else {
            continue;
        };
        let underlay: Ipv4Addr = raw.parse().map_err(|_| {
            ApiError::internal(format!("underlay {raw} on {} is not ipv4", host.meta.id))
        })?;
        if let Some(prev) = underlay_owner.insert(underlay, host.spec.node_name.clone()) {
            if prev != host.spec.node_name {
                return Err(ApiError::conflict(format!(
                    "underlay {underlay} is claimed by {prev} and {}",
                    host.spec.node_name
                )));
            }
        }
        out.push(Guest {
            port: port.clone(),
            underlay,
            node: host.spec.node_name.clone(),
        });
    }
    Ok(out)
}

/// Hosts that must see `net`: a guest on it, a floating binding whose port
/// is on the host, or the designated router that uses it as its external
/// network. The last two apply to provider and external overlay segments.
fn segment_hosts(
    net: &Network,
    kind: &Realised,
    guests: &[Guest],
    fips: &[FloatingIp],
    routers: &[Router],
    subnets: &[Subnet],
) -> Vec<SeenHost> {
    let mut nodes: BTreeMap<String, Ipv4Addr> = BTreeMap::new();
    for guest in guests.iter().filter(|g| g.port.spec.network == net.meta.id) {
        nodes.insert(guest.node.clone(), guest.underlay);
    }
    if matches!(kind, Realised::Provider(_) | Realised::External(_)) {
        for fip in fips.iter().filter(|f| f.spec.network == net.meta.id) {
            let Some((underlay, node, _)) = float_binding(fip, guests, routers, subnets) else {
                continue;
            };
            nodes.insert(node, underlay);
        }
        for holder in router_hosts(net, routers, subnets, guests) {
            nodes.insert(holder.node, holder.underlay);
        }
    }
    nodes
        .into_iter()
        .map(|(node, underlay)| SeenHost { underlay, node })
        .collect()
}

/// The host that receives the project router: the lowest underlay among
/// guests on each tenant network the router interfaces.
fn router_hosts(
    external: &Network,
    routers: &[Router],
    subnets: &[Subnet],
    guests: &[Guest],
) -> Vec<SeenHost> {
    let mut out = Vec::new();
    for router in routers {
        if router.spec.external_network.as_ref() != Some(&external.meta.id) {
            continue;
        }
        for sid in &router.spec.interfaces {
            let Some(sub) = subnets.iter().find(|s| &s.meta.id == sid) else {
                continue;
            };
            let here: Vec<&Guest> = guests
                .iter()
                .filter(|g| g.port.spec.network == sub.spec.network)
                .collect();
            let Some(underlay) = here.iter().map(|g| g.underlay).min() else {
                continue;
            };
            let Some(sample) = here.iter().find(|g| g.underlay == underlay) else {
                continue;
            };
            out.push(SeenHost {
                underlay,
                node: sample.node.clone(),
            });
        }
    }
    out
}

fn realise(
    underlay: Ipv4Addr,
    network: PlanNetwork,
    force_metadata: bool,
) -> ApiResult<SegmentPlan> {
    let planned = plan(
        &PlanHost {
            underlay: underlay.to_string(),
            underlay_mtu: 0,
        },
        &[network],
        &[],
    )
    .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    let mut segment = planned
        .networks
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("overlay plan had no network"))?;
    // Metadata is answered on the bridge of every host that has a tap,
    // including hosts that do not hold the router namespace.
    if force_metadata {
        segment.metadata = true;
    }
    Ok(segment)
}

fn port_endpoint(
    guest: &Guest,
    groups: &[SecurityGroup],
    ports: &[Port],
    subnets: &[Subnet],
) -> ApiResult<PlanPort> {
    let address = port_address(&guest.port, subnets).ok_or_else(|| {
        ApiError::unprocessable(format!("port {} has no address", guest.port.meta.id))
    })?;
    if guest.port.spec.mac.is_empty() {
        return Err(ApiError::unprocessable(format!(
            "port {} has no mac",
            guest.port.meta.id
        )));
    }
    let v6 = address.contains(':');
    let (ingress, egress) = if guest.port.spec.port_security {
        security_rules(&guest.port, v6, groups, ports)?
    } else {
        (Vec::new(), Vec::new())
    };
    // Every other fixed address is filtered too, with the rules of its own
    // family, so a dual-stack port is not left open on its second address.
    let mut extra = Vec::new();
    for ip in &guest.port.spec.fixed_ips {
        if ip.to_string() == address {
            continue;
        }
        let (ingress, egress) = if guest.port.spec.port_security {
            security_rules(&guest.port, ip.is_ipv6(), groups, ports)?
        } else {
            (Vec::new(), Vec::new())
        };
        extra.push(PortAddress {
            address: ip.to_string(),
            ingress,
            egress,
        });
    }
    Ok(PlanPort {
        id: guest.port.meta.id.to_string(),
        mac: guest.port.spec.mac.clone(),
        address,
        underlay: guest.underlay.to_string(),
        ingress,
        egress,
        extra,
    })
}

/// Address family of the subnet this port was allocated from, when known.
fn planned_family(port: &Port, subnets: &[Subnet]) -> Option<bool> {
    port.spec
        .subnet
        .as_ref()
        .and_then(|id| subnets.iter().find(|s| &s.meta.id == id))
        .map(|s| s.spec.cidr.addr().is_ipv4())
}

/// Prefer the subnet's family when the port has both. A lone IPv6 address
/// is still a port.
fn port_address(port: &Port, subnets: &[Subnet]) -> Option<String> {
    choose_address(port, planned_family(port, subnets))
}

fn choose_address(port: &Port, want_v4: Option<bool>) -> Option<String> {
    let ips = &port.spec.fixed_ips;
    if let Some(v4) = want_v4 {
        if let Some(ip) = ips.iter().find(|ip| ip.is_ipv4() == v4) {
            return Some(ip.to_string());
        }
    }
    ips.iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| ips.first())
        .map(|ip| ip.to_string())
}

/// Security group rules for one port, in the family of its planned address.
///
/// The helper filters traffic to that one address, so an IPv4 port gets the
/// IPv4 rules and an IPv6 port gets the IPv6 rules (applied with ip6tables).
/// A rule for the other family has nothing to match and is left out.
fn security_rules(
    port: &Port,
    v6: bool,
    groups: &[SecurityGroup],
    ports: &[Port],
) -> ApiResult<(Vec<SgRule>, Vec<SgRule>)> {
    let mut ingress = Vec::new();
    let mut egress = Vec::new();
    for id in &port.spec.security_groups {
        let Some(group) = groups.iter().find(|g| &g.meta.id == id) else {
            continue;
        };
        for rule in &group.spec.rules {
            let want = if v6 { EtherType::IPv6 } else { EtherType::IPv4 };
            if rule.ethertype != want {
                continue;
            }
            let (proto, port_min, port_max) = match rule.protocol {
                Some(Protocol::Tcp) => {
                    let (min, max) = span(rule.port_range.map(|p| (p.min, p.max)));
                    ("tcp", min, max)
                }
                Some(Protocol::Udp) => {
                    let (min, max) = span(rule.port_range.map(|p| (p.min, p.max)));
                    ("udp", min, max)
                }
                Some(Protocol::Icmp) => ("icmp", 0, 0),
                Some(Protocol::Any) | None => ("any", 0, 0),
            };
            let cidrs = match &rule.remote {
                Remote::Any if v6 => vec!["::/0".to_string()],
                Remote::Any => vec!["0.0.0.0/0".to_string()],
                Remote::Cidr(net) => {
                    let text = net.to_string();
                    if text.contains(':') != v6 {
                        continue;
                    }
                    vec![text]
                }
                Remote::Group(gid) => member_cidrs(gid, v6, ports),
            };
            for cidr in cidrs {
                let item = SgRule {
                    proto: proto.to_string(),
                    port_min,
                    port_max,
                    cidr,
                };
                match rule.direction {
                    Direction::Ingress => ingress.push(item),
                    Direction::Egress => egress.push(item),
                }
            }
        }
    }
    Ok((ingress, egress))
}

fn span(range: Option<(u16, u16)>) -> (u16, u16) {
    match range {
        Some((min, max)) if min > 0 && max >= min => (min, max),
        _ => (1, 65535),
    }
}

/// Host prefixes of every port in `group`, in one address family.
fn member_cidrs(group: &Id, v6: bool, ports: &[Port]) -> Vec<String> {
    let mut out = Vec::new();
    for port in ports {
        if !port.spec.security_groups.iter().any(|g| g == group) {
            continue;
        }
        for ip in &port.spec.fixed_ips {
            match ip {
                IpAddr::V4(_) if !v6 => out.push(format!("{ip}/32")),
                IpAddr::V6(_) if v6 => out.push(format!("{ip}/128")),
                _ => {}
            }
        }
    }
    out
}

fn routers_on(
    network: &Id,
    networks: &[Network],
    subnets: &[Subnet],
    routers: &[Router],
    ports: &[Port],
    fips: &[FloatingIp],
    bound: &[&Guest],
) -> ApiResult<Vec<PlanRouter>> {
    let on_net: Vec<&Subnet> = subnets
        .iter()
        .filter(|s| &s.spec.network == network)
        .collect();
    let mut out = Vec::new();
    for router in routers {
        let legs: Vec<&Subnet> = on_net
            .iter()
            .copied()
            .filter(|s| router.spec.interfaces.iter().any(|id| id == &s.meta.id))
            .collect();
        if legs.is_empty() {
            continue;
        }
        if legs.len() > 1 {
            return Err(ApiError::unprocessable(format!(
                "router {} has more than one subnet on {network}",
                router.meta.id
            )));
        }
        let subnet = legs[0];
        // IPv6 is a real leg. Only the shared host-NAT prefix is skipped,
        // and that network never reaches here.
        if subnet.spec.cidr.to_string() == "10.88.0.0/24" {
            continue;
        }
        let Some(gateway) = subnet.spec.gateway else {
            continue;
        };
        let external_ip = external_for(router, subnet, subnets);
        let dnat = if external_ip.as_ref().is_some_and(|ip| ip.contains(':')) {
            // IPv6 SNAT does not rewrite an IPv4 floating address.
            Vec::new()
        } else {
            fips.iter()
                .filter(|f| {
                    f.spec.port.as_ref().is_some_and(|p| {
                        bound.iter().any(|b| &b.port.meta.id == p)
                            && (f.status.router.as_ref() == Some(&router.meta.id)
                                || f.status.router.is_none())
                    })
                })
                .filter(|f| {
                    networks
                        .iter()
                        .find(|n| n.meta.id == f.spec.network)
                        .is_none_or(|n| !follows_port(n, subnets))
                })
                .filter_map(|f| {
                    let floating = f.spec.address.filter(|a| a.is_ipv4())?.to_string();
                    if external_ip.as_ref() == Some(&floating) {
                        return None;
                    }
                    let fixed = f
                        .status
                        .fixed_ip
                        .filter(|a| a.is_ipv4())
                        .map(|a| a.to_string())
                        .or_else(|| {
                            ports
                                .iter()
                                .find(|p| Some(&p.meta.id) == f.spec.port.as_ref())
                                .and_then(|p| {
                                    p.spec
                                        .fixed_ips
                                        .iter()
                                        .find(|ip| ip.is_ipv4())
                                        .map(|ip| ip.to_string())
                                })
                        })?;
                    Some(DnatRule {
                        id: f.meta.id.to_string(),
                        floating,
                        fixed,
                    })
                })
                .collect()
        };
        out.push(PlanRouter {
            id: router.meta.id.to_string(),
            gateway: gateway.to_string(),
            prefix: subnet.spec.cidr.prefix_len(),
            cidr: subnet.spec.cidr.to_string(),
            external_ip: external_ip.clone(),
            external_dev: if external_ip.is_some() {
                "external".into()
            } else {
                String::new()
            },
            dnat,
            metadata: true,
            dhcp: dhcp_leases(subnet, bound),
        });
    }
    Ok(out)
}

/// Leases for the designated router host. Other hosts send no router, so
/// they send no leases.
fn dhcp_leases(subnet: &Subnet, bound: &[&Guest]) -> Vec<DhcpLease> {
    if !subnet.spec.enable_dhcp {
        return Vec::new();
    }
    let dns: Vec<String> = subnet
        .spec
        .dns_nameservers
        .iter()
        .map(ToString::to_string)
        .collect();
    let routes: Vec<DhcpRoute> = subnet
        .spec
        .host_routes
        .iter()
        .map(|route| DhcpRoute {
            destination: route.destination.to_string(),
            nexthop: route.nexthop.to_string(),
        })
        .collect();
    let mut leases = Vec::new();
    for guest in bound {
        if !on_subnet(&guest.port, subnet) {
            continue;
        }
        let Some(address) = address_on_subnet(&guest.port, subnet) else {
            continue;
        };
        if guest.port.spec.mac.is_empty() {
            continue;
        }
        leases.push(DhcpLease {
            mac: guest.port.spec.mac.clone(),
            address,
            dns: dns.clone(),
            routes: routes.clone(),
        });
    }
    leases
}

fn on_subnet(port: &Port, subnet: &Subnet) -> bool {
    port.spec.subnet.as_ref() == Some(&subnet.meta.id)
        || port
            .spec
            .fixed_ips
            .iter()
            .any(|ip| subnet.spec.cidr.contains(ip))
}

fn address_on_subnet(port: &Port, subnet: &Subnet) -> Option<String> {
    let want_v4 = subnet.spec.cidr.addr().is_ipv4();
    port.spec
        .fixed_ips
        .iter()
        .find(|ip| ip.is_ipv4() == want_v4 && subnet.spec.cidr.contains(*ip))
        .map(|ip| ip.to_string())
}

fn attach_float_ports(
    ports: &mut Vec<PlanPort>,
    floating: &[PlanFloat],
    guests: &[Guest],
    groups: &[SecurityGroup],
    all_ports: &[Port],
    subnets: &[Subnet],
) -> ApiResult<()> {
    for float in floating {
        if ports.iter().any(|port| port.address == float.fixed) {
            continue;
        }
        let Some(guest) = guests.iter().find(|guest| {
            guest
                .port
                .spec
                .fixed_ips
                .iter()
                .any(|ip| ip.to_string() == float.fixed)
        }) else {
            continue;
        };
        if ports
            .iter()
            .any(|port| port.id == guest.port.meta.id.to_string())
        {
            continue;
        }
        let mut endpoint = port_endpoint(guest, groups, all_ports, subnets)?;
        endpoint.address = float.fixed.clone();
        ports.push(endpoint);
    }
    Ok(())
}

fn floats_for(
    net: &Network,
    underlay: Ipv4Addr,
    guests: &[Guest],
    fips: &[FloatingIp],
    routers: &[Router],
    subnets: &[Subnet],
) -> Vec<PlanFloat> {
    if !follows_port(net, subnets) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for fip in fips.iter().filter(|f| f.spec.network == net.meta.id) {
        let Some((host, _, float)) = float_binding(fip, guests, routers, subnets) else {
            continue;
        };
        if host == underlay {
            out.push(float);
        }
    }
    out
}

/// `Some` when this floating address is installed on the external segment of
/// the host that owns the guest port. An address equal to the router
/// external ip is never emitted.
fn float_binding(
    fip: &FloatingIp,
    guests: &[Guest],
    routers: &[Router],
    subnets: &[Subnet],
) -> Option<(Ipv4Addr, String, PlanFloat)> {
    let port_id = fip.spec.port.as_ref()?;
    let guest = guests.iter().find(|g| &g.port.meta.id == port_id)?;
    let floating = fip.spec.address?.to_string();
    if collides_with_router_ip(fip, &floating, &guest.port, routers, subnets) {
        return None;
    }
    let fixed = fip
        .status
        .fixed_ip
        .map(|ip| ip.to_string())
        .or_else(|| port_address(&guest.port, subnets))?;
    Some((
        guest.underlay,
        guest.node.clone(),
        PlanFloat {
            id: fip.meta.id.to_string(),
            floating,
            fixed,
        },
    ))
}

fn collides_with_router_ip(
    fip: &FloatingIp,
    floating: &str,
    port: &Port,
    routers: &[Router],
    subnets: &[Subnet],
) -> bool {
    routers.iter().any(|router| {
        let named = fip.status.router.as_ref() == Some(&router.meta.id);
        let on_port = router.spec.interfaces.iter().any(|sid| {
            subnets.iter().any(|subnet| {
                &subnet.meta.id == sid
                    && (port.spec.subnet.as_ref() == Some(&subnet.meta.id)
                        || port
                            .spec
                            .fixed_ips
                            .iter()
                            .any(|ip| subnet.spec.cidr.contains(ip)))
            })
        });
        (named || on_port)
            && router
                .status
                .external_ip
                .map(|ip| ip.to_string())
                .as_deref()
                == Some(floating)
    })
}

fn shared_nat(address: &str) -> bool {
    let Ok(ip) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    let o = ip.octets();
    o[0] == 10 && o[1] == 88 && o[2] == 0
}

fn store_err(e: pistack_store::StoreError) -> ApiError {
    ApiError::internal(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nic::ensure_ports;
    use crate::types::{new_host, new_machine, MachineSpec};
    use fabric::{
        Client, FloatingIpSpec, FloatingIpStatus, MockBackend, NetworkSpec, PortSpec, RouterSpec,
        Rule, SecurityGroupSpec, SubnetSpec,
    };
    use pistack_types::{Meta, Phase, Resources};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn host(name: &str, underlay: &str) -> Host {
        let mut host = new_host(
            name,
            name,
            Resources {
                vcpus: 4,
                memory_mib: 8192,
                disk_gib: 100,
                gpus: 0,
            },
            vec!["kvm".into(), "libvirt".into()],
            BTreeMap::new(),
        );
        host.spec.underlay_ip = Some(underlay.into());
        host.status.phase = Phase::Ready;
        host
    }

    #[test]
    fn a_hostnat_network_stays_on_the_host_bridge() {
        let mut net = fabric::Network {
            meta: pistack_types::Meta::new(kind::NETWORK, "app", None),
            spec: fabric::NetworkSpec::default(),
            status: fabric::NetworkStatus {
                segment_id: Some(7),
                datapath: Some("hostnat".into()),
                ..fabric::NetworkStatus::default()
            },
        };
        let stayed = segment_for_nic(&net, "10.90.0.2").unwrap();
        assert!(stayed.is_none());
        net.status.datapath = Some("overlay".into());
        // A parallel test may or may not select the overlay backend. Host NAT
        // is the case that must hold either way: the assertion above.
        let _ = segment_for_nic(&net, "10.90.0.2");
    }

    #[tokio::test]
    async fn two_projects_with_the_same_cidr_get_two_segments_and_the_peer_is_not_flooded() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
        let a = host("a", "10.1.0.1");
        let b = host("b", "10.1.0.2");
        store.collection::<Host>().create(a.clone()).unwrap();
        store.collection::<Host>().create(b.clone()).unwrap();

        let project = Id::new(kind::PROJECT);
        let other = Id::new(kind::PROJECT);
        let (net_a, port_a) = tenant(
            &client,
            &store,
            "app",
            &project,
            &a,
            "10.90.0.0/24",
            7,
            "10.66.66.84",
            "10.66.66.85",
        )
        .await;
        let (net_b, _) = tenant(
            &client,
            &store,
            "other",
            &other,
            &a,
            "10.90.0.0/24",
            8,
            "10.66.66.86",
            "10.66.66.87",
        )
        .await;

        // Second guest of the first project, on the other host.
        let machine_b = placed("box-b", &project, "app", &b);
        store
            .collection::<Machine>()
            .create(machine_b.clone())
            .unwrap();
        ensure_ports(&store, &machine_b).await.unwrap();

        let plans = all_plans(&store).unwrap();
        let plan_a = plan_for(&plans, "a");
        let plan_b = plan_for(&plans, "b");
        assert_eq!(plan_a.networks.len(), 2);
        assert_eq!(plan_b.networks.len(), 1);
        let seg_a = plan_a.networks.iter().find(|n| n.id == net_a).unwrap();
        let seg_other = plan_a.networks.iter().find(|n| n.id == net_b).unwrap();
        assert_ne!(seg_a.vni, seg_other.vni);
        assert_ne!(seg_a.bridge, seg_other.bridge);
        assert!(seg_a.bridge.starts_with("po"));
        assert!(seg_other.bridge.starts_with("po"));
        assert!(!seg_a.bridge.starts_with("pi"));
        assert_ne!(seg_a.routers[0].netns, seg_other.routers[0].netns);
        assert_eq!(seg_a.routers[0].interfaces[0].gateway, "10.90.0.1");
        assert_eq!(seg_other.routers[0].interfaces[0].gateway, "10.90.0.1");
        assert!(seg_a.metadata);
        assert!(!seg_a.peers.contains(&"0.0.0.0".to_string()));
        assert!(seg_a.peers.contains(&"10.1.0.2".to_string()));
        assert!(!seg_a.peers.contains(&"10.1.0.1".to_string()));
        assert!(seg_a
            .ports
            .iter()
            .any(|p| p.id == port_a && p.underlay == "10.1.0.1"));
        let remote = plan_b.networks[0].clone();
        assert!(
            remote.routers.is_empty(),
            "the router stays on the lowest underlay"
        );
        assert!(remote.metadata);
        assert!(remote.ports.iter().any(|p| p.underlay == "10.1.0.1"));
        assert!(remote.peers.contains(&"10.1.0.1".to_string()));
        assert!(seg_a.routers[0]
            .dnat
            .iter()
            .any(|d| d.floating == "10.66.66.85"));
    }

    fn plan_for(plans: &[(String, Value)], node: &str) -> OverlayPlan {
        let (_, value) = plans.iter().find(|(n, _)| n == node).unwrap();
        serde_json::from_value(value.clone()).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn tenant(
        client: &Client,
        store: &Store,
        name: &str,
        project: &Id,
        host: &Host,
        cidr: &str,
        vni: u32,
        external_ip: &str,
        floating: &str,
    ) -> (String, String) {
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, name, Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        store
            .collection::<Network>()
            .modify(&net.meta.id, |n| {
                n.status.segment_id = Some(vni);
                Ok(())
            })
            .unwrap();
        let sub = client
            .create_subnet(
                Meta::new(kind::SUBNET, format!("{name}-net"), Some(project.clone())),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: cidr.parse().unwrap(),
                    gateway: Some("10.90.0.1".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap();
        let machine = placed(name, project, name, host);
        store
            .collection::<Machine>()
            .create(machine.clone())
            .unwrap();
        ensure_ports(store, &machine).await.unwrap();
        let port = client
            .ports_of_device(&machine.meta.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        let ext_spec = NetworkSpec {
            external: true,
            ..Default::default()
        };
        let ext = client
            .create_network(
                Meta::new(kind::NETWORK, format!("{name}-ext"), Some(project.clone())),
                ext_spec,
            )
            .unwrap();
        client
            .create_subnet(
                Meta::new(kind::SUBNET, format!("{name}-pool"), Some(project.clone())),
                SubnetSpec {
                    network: ext.meta.id.clone(),
                    cidr: "10.66.66.80/28".parse().unwrap(),
                    gateway: Some("10.66.66.81".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: false,
                    host_routes: vec![],
                },
            )
            .unwrap();
        let router = client
            .create_router(
                Meta::new(kind::ROUTER, format!("{name}-edge"), Some(project.clone())),
                RouterSpec {
                    external_network: Some(ext.meta.id.clone()),
                    interfaces: vec![sub.meta.id],
                    routes: vec![],
                },
            )
            .unwrap();
        store
            .collection::<Router>()
            .modify(&router.meta.id, |r| {
                r.status.external_ip = Some(external_ip.parse().unwrap());
                Ok(())
            })
            .unwrap();
        let fip_addr: std::net::IpAddr = floating.parse().unwrap();
        store
            .collection::<FloatingIp>()
            .create(FloatingIp {
                meta: Meta::new(
                    kind::FLOATING_IP,
                    format!("{name}-vip"),
                    Some(project.clone()),
                ),
                spec: FloatingIpSpec {
                    network: ext.meta.id,
                    address: Some(fip_addr),
                    port: Some(port.meta.id.clone()),
                },
                status: FloatingIpStatus {
                    fixed_ip: Some(port.spec.fixed_ips[0]),
                    router: Some(router.meta.id),
                    ..FloatingIpStatus::default()
                },
            })
            .unwrap();
        (net.meta.id.to_string(), port.meta.id.to_string())
    }

    fn placed(name: &str, project: &Id, network: &str, host: &Host) -> Machine {
        let mut machine = new_machine(
            name,
            Some(project.clone()),
            MachineSpec {
                size: "s.1x1".into(),
                image: "ubuntu-24.04".into(),
                networks: vec![network.into()],
                ..MachineSpec::default()
            },
        );
        machine.status.host = Some(host.meta.id.clone());
        machine.status.phase = Phase::Ready;
        machine
    }

    #[tokio::test]
    async fn group_members_expand_to_the_other_port_and_an_outsider_is_absent() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
        let a = host("a", "10.1.0.1");
        store.collection::<Host>().create(a.clone()).unwrap();
        let project = Id::new(kind::PROJECT);
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "app", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        store
            .collection::<Network>()
            .modify(&net.meta.id, |n| {
                n.status.segment_id = Some(7);
                Ok(())
            })
            .unwrap();
        client
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
        let group = client
            .create_security_group(
                Meta::new(kind::SECGROUP, "web", Some(project.clone())),
                SecurityGroupSpec { rules: vec![] },
            )
            .unwrap();
        client
            .add_rule(
                &group.meta.id,
                Rule::new(Direction::Ingress, EtherType::IPv4)
                    .protocol(Protocol::Tcp)
                    .ports(22, 22)
                    .remote(Remote::Group(group.meta.id.clone())),
            )
            .unwrap();
        client
            .create_security_group(
                Meta::new(kind::SECGROUP, "other", Some(project.clone())),
                SecurityGroupSpec { rules: vec![] },
            )
            .unwrap();

        let mut members = Vec::new();
        for (name, sg) in [("one", "web"), ("two", "web"), ("out", "other")] {
            let mut machine = placed(name, &project, "app", &a);
            machine.spec.security_groups = vec![sg.into()];
            store
                .collection::<Machine>()
                .create(machine.clone())
                .unwrap();
            ensure_ports(&store, &machine).await.unwrap();
            let port = client
                .ports_of_device(&machine.meta.id)
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            members.push(port.meta.id.to_string());
        }

        let plan = plan_for(&all_plans(&store).unwrap(), "a");
        let seg = plan
            .networks
            .iter()
            .find(|n| n.id == net.meta.id.to_string())
            .unwrap();
        let port = |id: &str| seg.ports.iter().find(|p| p.id == id).unwrap();
        let one = port(&members[0]);
        let two = port(&members[1]);
        let outsider = port(&members[2]);
        let peer = format!("{}/32", two.address);
        let other = format!("{}/32", outsider.address);
        assert!(one.ingress.iter().any(|rule| rule.cidr == peer));
        assert!(two
            .ingress
            .iter()
            .any(|rule| rule.cidr == format!("{}/32", one.address)));
        assert!(one.ingress.iter().all(|rule| rule.cidr != other));
        assert!(two.ingress.iter().all(|rule| rule.cidr != other));
    }

    #[tokio::test]
    async fn floating_ip_follows_the_guest_onto_the_provider_network() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
        let router_host = host("a", "10.1.0.1");
        let guest_host = host("b", "10.1.0.2");
        store
            .collection::<Host>()
            .create(router_host.clone())
            .unwrap();
        store
            .collection::<Host>()
            .create(guest_host.clone())
            .unwrap();
        let project = Id::new(kind::PROJECT);

        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "app", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        store
            .collection::<Network>()
            .modify(&net.meta.id, |n| {
                n.status.segment_id = Some(7);
                Ok(())
            })
            .unwrap();
        let dns = "9.9.9.9";
        let sub = client
            .create_subnet(
                Meta::new(kind::SUBNET, "app-net", Some(project.clone())),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: "10.90.0.0/24".parse().unwrap(),
                    gateway: Some("10.90.0.1".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![dns.parse().unwrap()],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap();

        // A guest on the lower underlay keeps the router there. The floating
        // address belongs to the guest on the higher underlay.
        let anchor = placed("anchor", &project, "app", &router_host);
        store
            .collection::<Machine>()
            .create(anchor.clone())
            .unwrap();
        ensure_ports(&store, &anchor).await.unwrap();
        let guest = placed("guest", &project, "app", &guest_host);
        store.collection::<Machine>().create(guest.clone()).unwrap();
        ensure_ports(&store, &guest).await.unwrap();
        let guest_port = client
            .ports_of_device(&guest.meta.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        let mut ext_spec = NetworkSpec {
            external: true,
            ..Default::default()
        };
        ext_spec.kind = NetworkKind::Provider {
            physnet: "eno1".into(),
            vlan: Some(20),
        };
        let ext = client
            .create_network(
                Meta::new(kind::NETWORK, "edge", Some(project.clone())),
                ext_spec,
            )
            .unwrap();
        client
            .create_subnet(
                Meta::new(kind::SUBNET, "edge-pool", Some(project.clone())),
                SubnetSpec {
                    network: ext.meta.id.clone(),
                    cidr: "10.66.66.80/28".parse().unwrap(),
                    gateway: Some("10.66.66.81".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: false,
                    host_routes: vec![],
                },
            )
            .unwrap();
        let router = client
            .create_router(
                Meta::new(kind::ROUTER, "edge", Some(project.clone())),
                RouterSpec {
                    external_network: Some(ext.meta.id.clone()),
                    interfaces: vec![sub.meta.id],
                    routes: vec![],
                },
            )
            .unwrap();
        store
            .collection::<Router>()
            .modify(&router.meta.id, |r| {
                r.status.external_ip = Some("10.66.66.84".parse().unwrap());
                Ok(())
            })
            .unwrap();
        let fip = store
            .collection::<FloatingIp>()
            .create(FloatingIp {
                meta: Meta::new(kind::FLOATING_IP, "guest-vip", Some(project.clone())),
                spec: FloatingIpSpec {
                    network: ext.meta.id.clone(),
                    address: Some("10.66.66.85".parse().unwrap()),
                    port: Some(guest_port.meta.id.clone()),
                },
                status: FloatingIpStatus {
                    fixed_ip: Some(guest_port.spec.fixed_ips[0]),
                    router: Some(router.meta.id),
                    ..FloatingIpStatus::default()
                },
            })
            .unwrap();

        let plans = all_plans(&store).unwrap();
        let on_guest = plan_for(&plans, "b");
        let on_router = plan_for(&plans, "a");
        let provider = on_guest
            .networks
            .iter()
            .find(|n| n.id == ext.meta.id.to_string())
            .unwrap();
        assert_eq!(provider.vni, 0);
        assert!(provider.routers.is_empty());
        assert!(provider
            .floating
            .iter()
            .any(|f| f.id == fip.meta.id.to_string()));
        assert!(provider
            .floating
            .iter()
            .all(|f| f.floating != "10.66.66.84"));
        let tenant = on_router
            .networks
            .iter()
            .find(|n| n.id == net.meta.id.to_string())
            .unwrap();
        assert!(tenant.routers[0]
            .dnat
            .iter()
            .all(|d| d.floating != "10.66.66.85"));
        let lease = tenant.routers[0].interfaces[0]
            .dhcp
            .iter()
            .find(|lease| lease.mac == guest_port.spec.mac)
            .unwrap();
        assert!(lease.dns.iter().any(|server| server == dns));

        let scoped = plans_for_machine(&store, &guest).unwrap();
        let scoped_guest = plan_for(&scoped, "b");
        assert!(
            scoped_guest
                .networks
                .iter()
                .any(|n| n.id == ext.meta.id.to_string()),
            "the external segment stays even though the machine does not name it"
        );
    }

    #[tokio::test]
    async fn ipv6_router_external_snat_is_a_private_ula() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone(), Arc::new(MockBackend::new()));
        let a = host("a", "10.1.0.1");
        store.collection::<Host>().create(a.clone()).unwrap();
        let project = Id::new(kind::PROJECT);
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "app6", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        store
            .collection::<Network>()
            .modify(&net.meta.id, |n| {
                n.status.segment_id = Some(11);
                Ok(())
            })
            .unwrap();
        let sub = client
            .create_subnet(
                Meta::new(kind::SUBNET, "app6-net", Some(project.clone())),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: "fd00:90::/64".parse().unwrap(),
                    gateway: Some("fd00:90::1".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap();
        let machine = placed("box6", &project, "app6", &a);
        store
            .collection::<Machine>()
            .create(machine.clone())
            .unwrap();
        ensure_ports(&store, &machine).await.unwrap();
        let router = client
            .create_router(
                Meta::new(kind::ROUTER, "app6-edge", Some(project.clone())),
                RouterSpec {
                    external_network: None,
                    interfaces: vec![sub.meta.id],
                    routes: vec![],
                },
            )
            .unwrap();
        store
            .collection::<Router>()
            .modify(&router.meta.id, |r| {
                r.status.external_ip = Some("fd00:66::9".parse().unwrap());
                Ok(())
            })
            .unwrap();

        let plan = plan_for(&all_plans(&store).unwrap(), "a");
        assert_eq!(plan.host, "10.1.0.1");
        let seg = plan
            .networks
            .iter()
            .find(|n| n.id == net.meta.id.to_string())
            .unwrap();
        assert_eq!(seg.routers.len(), 1);
        assert!(seg.routers[0].snat);
        assert_eq!(seg.routers[0].external_ip.as_deref(), Some("fd00:66::9"));
        assert_eq!(seg.routers[0].interfaces[0].cidr, "fd00:90::/64");
        assert_eq!(seg.routers[0].interfaces[0].gateway, "fd00:90::1");
        assert!(seg.routers[0].dnat.is_empty());

        store
            .collection::<Router>()
            .modify(&router.meta.id, |r| {
                r.status.external_ip = Some("2001:db8::9".parse().unwrap());
                Ok(())
            })
            .unwrap();
        let plan = plan_for(&all_plans(&store).unwrap(), "a");
        let seg = plan
            .networks
            .iter()
            .find(|n| n.id == net.meta.id.to_string())
            .unwrap();
        assert!(!seg.routers[0].snat);
        assert!(seg.routers[0].external_ip.is_none());
        assert_eq!(plan.host, "10.1.0.1");
    }

    fn sg_port(ips: &[&str], group: &Id) -> Port {
        Port {
            meta: Meta::new(kind::PORT, "p", None),
            spec: PortSpec {
                fixed_ips: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
                security_groups: vec![group.clone()],
                ..PortSpec::default()
            },
            status: Default::default(),
        }
    }

    #[test]
    fn ipv6_port_gets_ipv6_rules_and_ipv4_port_keeps_ipv4_rules() {
        let mut group = SecurityGroup {
            meta: Meta::new(kind::SECGROUP, "web", None),
            spec: SecurityGroupSpec { rules: vec![] },
            status: Default::default(),
        };
        let gid = group.meta.id.clone();
        group.spec.rules = vec![
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(22, 22),
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .protocol(Protocol::Tcp)
                .ports(443, 443),
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .protocol(Protocol::Icmp)
                .remote(Remote::Group(gid.clone())),
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .remote(Remote::Cidr("10.0.0.0/8".parse().unwrap())),
            Rule::new(Direction::Egress, EtherType::IPv6),
        ];
        let me = sg_port(&["10.90.0.5", "fd00::5"], &gid);
        let peer = sg_port(&["10.90.0.6", "fd00::6"], &gid);
        let ports = vec![me.clone(), peer];
        let groups = vec![group];

        let (ing, eg) = security_rules(&me, true, &groups, &ports).unwrap();
        assert!(ing
            .iter()
            .any(|r| r.proto == "tcp" && r.port_min == 443 && r.cidr == "::/0"));
        assert!(ing
            .iter()
            .any(|r| r.proto == "icmp" && r.cidr == "fd00::6/128"));
        assert!(ing.iter().all(|r| r.cidr.contains(':')), "{ing:?}");
        assert!(ing.iter().all(|r| r.port_min != 22));
        assert_eq!(eg.len(), 1);
        assert_eq!(eg[0].cidr, "::/0");

        let (ing4, eg4) = security_rules(&me, false, &groups, &ports).unwrap();
        assert_eq!(ing4.len(), 1);
        assert_eq!(ing4[0].port_min, 22);
        assert_eq!(ing4[0].cidr, "0.0.0.0/0");
        assert!(eg4.is_empty());
    }

    #[test]
    fn dual_stack_port_plans_rules_for_both_addresses() {
        let mut group = SecurityGroup {
            meta: Meta::new(kind::SECGROUP, "web", None),
            spec: SecurityGroupSpec { rules: vec![] },
            status: Default::default(),
        };
        let gid = group.meta.id.clone();
        group.spec.rules = vec![
            Rule::new(Direction::Ingress, EtherType::IPv4)
                .protocol(Protocol::Tcp)
                .ports(22, 22),
            Rule::new(Direction::Ingress, EtherType::IPv6)
                .protocol(Protocol::Tcp)
                .ports(443, 443),
        ];
        let mut port = sg_port(&["10.90.0.5", "fd00::5"], &gid);
        port.spec.mac = "52:54:00:00:00:55".into();
        let guest = Guest {
            port: port.clone(),
            underlay: "10.1.0.1".parse().unwrap(),
            node: "a".into(),
        };
        let groups = vec![group];
        let got = port_endpoint(&guest, &groups, &[port], &[]).unwrap();
        assert_eq!(got.address, "10.90.0.5");
        assert_eq!(got.ingress.len(), 1);
        assert_eq!(got.ingress[0].port_min, 22);
        assert_eq!(got.extra.len(), 1);
        assert_eq!(got.extra[0].address, "fd00::5");
        assert_eq!(got.extra[0].ingress.len(), 1);
        assert_eq!(got.extra[0].ingress[0].port_min, 443);
        assert_eq!(got.extra[0].ingress[0].cidr, "::/0");
    }
}
