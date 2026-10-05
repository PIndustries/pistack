// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Tenant overlay plan.
//!
//! Each overlay network gets a bridge and a static VXLAN device. The VNI is
//! the segment id and the UDP destination port is [`VXLAN_PORT`]. A provider
//! network keeps the bridge but attaches a host physnet instead of a VXLAN
//! device, so that segment has an empty vxlan name and no underlay peers.
//! Each router gets one network namespace, even with several interfaces, so
//! two projects can both hold the same gateway address. The plan is data
//! only: it does not spawn processes or change the host. The agent applies it.
//!
//! Names are `po`/`vx`/`pv` plus 12 hex digits of the network id, `ns`/`c`/`d`
//! plus 12 hex digits of the router id, `a`/`b` plus 6 hex digits of the
//! network and 6 of the router, and `nf`/`e`/`f` plus 12 hex digits of a
//! floating id. They are never `pistack0` and never start with `pi` (those
//! are host-NAT bridge names). The caller's external device is recorded and
//! not given an address.

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use thiserror::Error;

/// Schema version written into [`OverlayPlan::version`].
pub const PLAN_VERSION: u32 = 1;

/// Linux VXLAN destination port.
pub const VXLAN_PORT: u16 = 4789;

const MAX_VNI: u32 = 16_777_215;

/// One host's overlay: a segment per network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayPlan {
    pub version: u32,
    /// This host's underlay IPv4.
    pub host: String,
    pub networks: Vec<SegmentPlan>,
}

/// One overlay network as realised on this host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentPlan {
    pub id: String,
    pub vni: u32,
    /// Payload MTU. Never 0. Default is the underlay MTU minus 50.
    pub mtu: u16,
    pub bridge: String,
    /// `vx` plus 12 hex digits, or empty when [`Self::provider`] is set.
    /// An empty name means the agent must not create a VXLAN device.
    pub vxlan: String,
    /// This host's underlay IPv4.
    pub local: String,
    /// Other underlay IPv4s, sorted and unique. Never this host, never
    /// `0.0.0.0`. Empty on a provider segment.
    pub peers: Vec<String>,
    pub ports: Vec<PortEndpoint>,
    /// Namespaces whose first attachment is this network. A router on several
    /// networks appears once and lists every interface.
    pub routers: Vec<RouterNamespace>,
    /// Answer metadata on this bridge even when the router namespace is on
    /// another host. Missing on older plans means false.
    #[serde(default)]
    pub metadata: bool,
    /// Host physnet attachment. Absent on a VXLAN segment and on older plans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderAttach>,
    /// Floating addresses hosted on this segment. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floating: Vec<FloatingLocal>,
}

/// A guest port, and the underlay host that owns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortEndpoint {
    pub id: String,
    pub mac: String,
    /// Guest address, no prefix. IPv4 or IPv6.
    pub address: String,
    /// Host that owns this port.
    pub underlay: String,
    pub ingress: Vec<SgRule>,
    pub egress: Vec<SgRule>,
}

/// One security-group match. `proto` is `tcp`, `udp`, `icmp`, or `any`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SgRule {
    pub proto: String,
    pub port_min: u16,
    pub port_max: u16,
    pub cidr: String,
}

/// One router namespace. The gateway address lives on `veth_ns`, not on the bridge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterNamespace {
    pub id: String,
    pub netns: String,
    pub interfaces: Vec<RouterInterface>,
    /// Root end of the external veth. `c` plus 12 hex digits of the router id.
    pub external_veth_root: String,
    /// Namespace end of the external veth. `d` plus the same 12 hex digits.
    pub external_veth_ns: String,
    /// Router address on the external network. Installed as a host prefix
    /// (`/32` or `/128`) in the namespace. IPv6 is unique-local (`fc00::/7`).
    pub external_ip: Option<String>,
    /// Recorded only. The agent must not move, enslave, or address this device.
    /// Empty, and left out of the document, when the router has no external
    /// address: that router gets no external veth.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_dev: String,
    /// True only when [`Self::external_ip`] is set.
    pub snat: bool,
    pub dnat: Vec<DnatRule>,
    pub metadata: bool,
}

/// A router leg on one tenant network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterInterface {
    pub network_id: String,
    /// Bridge side, in the root namespace, enslaved to the network bridge.
    pub veth_bridge: String,
    /// Namespace side. Holds the gateway.
    pub veth_ns: String,
    pub gateway: String,
    pub prefix: u8,
    pub cidr: String,
    /// Leases answered on this leg. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dhcp: Vec<DhcpLease>,
}

/// A floating address DNATed to a guest fixed address inside the router namespace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnatRule {
    pub id: String,
    pub floating: String,
    pub fixed: String,
}

/// One DHCP route handed to a guest with its lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DhcpRoute {
    pub destination: String,
    pub nexthop: String,
}

/// One address handed out on a router leg.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DhcpLease {
    pub mac: String,
    pub address: String,
    pub dns: Vec<String>,
    pub routes: Vec<DhcpRoute>,
}

/// Caller-supplied physnet for a provider network. Not a planned device.
#[derive(Clone, Debug)]
pub struct PlanProvider {
    pub physnet: String,
    pub vlan: Option<u16>,
}

/// How a provider segment attaches to a host physnet.
///
/// `link` is `pv` plus 12 hex digits of the network id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttach {
    pub physnet: String,
    pub vlan: Option<u16>,
    pub link: String,
}

/// A floating address to host on the network that lists it.
#[derive(Clone, Debug)]
pub struct PlanFloat {
    pub id: String,
    pub floating: String,
    pub fixed: String,
}

/// A floating address hosted in its own namespace on this segment.
///
/// `netns` is `nf` plus 12 hex digits of the floating id. `veth_bridge` is
/// `e` plus those digits, and `veth_ns` is `f` plus those digits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatingLocal {
    pub id: String,
    pub floating: String,
    pub fixed: String,
    pub netns: String,
    pub veth_bridge: String,
    pub veth_ns: String,
}

/// The host the plan is for.
#[derive(Clone, Debug)]
pub struct PlanHost {
    pub underlay: String,
    /// `0` means 1500.
    pub underlay_mtu: u16,
}

/// One overlay network to realise.
#[derive(Clone, Debug)]
pub struct PlanNetwork {
    pub id: String,
    pub vni: u32,
    pub mtu: u16,
    pub ports: Vec<PlanPort>,
    pub routers: Vec<PlanRouter>,
    /// When set, this network attaches a physnet and does not get a VXLAN device.
    pub provider: Option<PlanProvider>,
    pub floating: Vec<PlanFloat>,
}

/// A guest port supplied to [`plan`].
#[derive(Clone, Debug)]
pub struct PlanPort {
    pub id: String,
    pub mac: String,
    pub address: String,
    pub underlay: String,
    pub ingress: Vec<SgRule>,
    pub egress: Vec<SgRule>,
}

/// A router leg on the network that contains it.
///
/// The same `id` on two networks becomes one [`RouterNamespace`] with two
/// [`RouterInterface`]s.
#[derive(Clone, Debug)]
pub struct PlanRouter {
    pub id: String,
    pub gateway: String,
    pub prefix: u8,
    pub cidr: String,
    pub external_ip: Option<String>,
    pub external_dev: String,
    pub dnat: Vec<DnatRule>,
    pub metadata: bool,
    /// Input only. Copied onto this leg's [`RouterInterface`]. Not serialized.
    pub dhcp: Vec<DhcpLease>,
}

/// Why [`plan`] refused a document.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("{0}")]
    Ip(String),
    #[error("vni {0} is outside 1..=16777215")]
    Vni(u32),
    #[error("overlay mtu would be 0")]
    Mtu,
    #[error("{0} does not yield 12 hex digits")]
    Id(String),
    #[error("refusing device name {0}")]
    Name(String),
    #[error("mac {0} is not six hex octets")]
    Mac(String),
    #[error("{0}")]
    Invalid(String),
}

struct PendingRouter {
    at: usize,
    ns: RouterNamespace,
}

/// Twelve lowercase hex digits used in overlay device names.
///
/// Split on the first `-`. When the left side is all ASCII letters and the
/// right side is non-empty, only the right side is used. Otherwise the whole
/// id is used. Non-hex characters are skipped.
/// `net-01a10318-a507-74fd-a86e-5acbda974722` yields `01a10318a507`.
pub fn device_hex(id: &str) -> Result<String, PlanError> {
    let source = match id.split_once('-') {
        Some((kind, rest))
            if !rest.is_empty() && kind.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            rest
        }
        _ => id,
    };
    let mut hex = String::with_capacity(12);
    for c in source.chars() {
        if c.is_ascii_hexdigit() {
            hex.push(c.to_ascii_lowercase());
            if hex.len() == 12 {
                return Ok(hex);
            }
        }
    }
    Err(PlanError::Id(id.to_string()))
}

/// Plan one host.
///
/// `peer_underlays` are other hosts on the underlay. A port whose underlay is
/// not this host is added too. This host is not a peer of itself. No process
/// is spawned.
pub fn plan(
    host: &PlanHost,
    networks: &[PlanNetwork],
    peer_underlays: &[String],
) -> Result<OverlayPlan, PlanError> {
    let local = unicast("underlay", &host.underlay)?;
    let local_s = local.to_string();
    let underlay_mtu = if host.underlay_mtu == 0 {
        1500
    } else {
        host.underlay_mtu
    };

    let mut peers = BTreeSet::new();
    for raw in peer_underlays {
        let ip = unicast("peer", raw)?;
        if ip != local {
            peers.insert(ip);
        }
    }
    let external_ips = external_ips(networks)?;

    let mut seen_net = BTreeSet::new();
    let mut seen_vni = BTreeSet::new();
    let mut seen_port = BTreeSet::new();
    let mut floating_ids = BTreeSet::new();
    let mut floating_addrs = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut segments = Vec::with_capacity(networks.len());
    let mut pending = Vec::new();

    for net in networks {
        if !seen_net.insert(net.id.clone()) {
            return Err(PlanError::Invalid(format!("duplicate network {}", net.id)));
        }
        // A provider segment is not VXLAN. VNI 0 is allowed, and its VNI is
        // not unique against other provider segments. Above the VXLAN max is
        // still refused.
        if net.provider.is_none() {
            if !(1..=MAX_VNI).contains(&net.vni) {
                return Err(PlanError::Vni(net.vni));
            }
            if !seen_vni.insert(net.vni) {
                return Err(PlanError::Invalid(format!("duplicate vni {}", net.vni)));
            }
        } else if net.vni > MAX_VNI {
            return Err(PlanError::Vni(net.vni));
        }
        let hex = device_hex(&net.id)?;
        let bridge = claim(&mut names, format!("po{hex}"), "po")?;
        let provider = match &net.provider {
            Some(spec) => Some(canonical_provider(spec, &hex, &mut names)?),
            None => None,
        };
        // No VXLAN name is claimed for a provider segment, so nothing installs one.
        let vxlan = if provider.is_some() {
            String::new()
        } else {
            claim(&mut names, format!("vx{hex}"), "vx")?
        };
        let mtu = overlay_mtu(net.mtu, underlay_mtu)?;
        let ports = plan_ports(net, local, &mut peers, &mut seen_port)?;
        let floating = plan_floating(
            net,
            &ports,
            &external_ips,
            &mut floating_ids,
            &mut floating_addrs,
            &mut names,
        )?;
        let at = segments.len();
        segments.push(SegmentPlan {
            id: net.id.clone(),
            vni: net.vni,
            mtu,
            bridge,
            vxlan,
            local: local_s.clone(),
            peers: Vec::new(),
            ports,
            routers: Vec::new(),
            metadata: net.routers.iter().any(|r| r.metadata),
            provider,
            floating,
        });
        for router in &net.routers {
            attach_router(&mut pending, &mut names, at, &net.id, &hex, router)?;
        }
    }

    let peer_list: Vec<String> = peers.iter().map(ToString::to_string).collect();
    for seg in &mut segments {
        if seg.provider.is_none() {
            seg.peers.clone_from(&peer_list);
        }
    }
    for mut router in pending {
        if router.ns.external_ip.is_none() {
            // No external address means no external veth, so no underlay to record.
            router.ns.external_dev.clear();
        }
        segments[router.at].routers.push(router.ns);
    }
    for seg in &segments {
        for ns in &seg.routers {
            v6_snat_agrees(ns)?;
        }
    }

    Ok(OverlayPlan {
        version: PLAN_VERSION,
        host: local_s,
        networks: segments,
    })
}

fn plan_ports(
    net: &PlanNetwork,
    local: Ipv4Addr,
    peers: &mut BTreeSet<Ipv4Addr>,
    seen_port: &mut BTreeSet<String>,
) -> Result<Vec<PortEndpoint>, PlanError> {
    let mut ports = Vec::with_capacity(net.ports.len());
    let mut seen_mac = BTreeSet::new();
    let mut seen_addr = BTreeSet::new();
    for port in &net.ports {
        if port.id.is_empty() {
            return Err(PlanError::Invalid("port id is empty".into()));
        }
        if !seen_port.insert(port.id.clone()) {
            return Err(PlanError::Invalid(format!("duplicate port {}", port.id)));
        }
        if port.underlay.is_empty() {
            return Err(PlanError::Invalid(format!(
                "port {} has an empty underlay",
                port.id
            )));
        }
        let owned = unicast("port underlay", &port.underlay)?;
        if owned != local {
            peers.insert(owned);
        }
        let mac = canonical_mac(&port.mac)?;
        if !seen_mac.insert(mac.clone()) {
            return Err(PlanError::Invalid(format!(
                "duplicate mac {mac} on {}",
                net.id
            )));
        }
        let address_ip = canonical_ip("port address", &port.address)?;
        if !seen_addr.insert(address_ip) {
            return Err(PlanError::Invalid(format!(
                "duplicate address {address_ip} on {}",
                net.id
            )));
        }
        ports.push(PortEndpoint {
            id: port.id.clone(),
            mac,
            address: address_ip.to_string(),
            underlay: owned.to_string(),
            ingress: canonical_rules(&port.ingress)?,
            egress: canonical_rules(&port.egress)?,
        });
    }
    Ok(ports)
}

fn attach_router(
    pending: &mut Vec<PendingRouter>,
    names: &mut BTreeSet<String>,
    at: usize,
    network_id: &str,
    net_hex: &str,
    router: &PlanRouter,
) -> Result<(), PlanError> {
    if pending.iter().any(|p| {
        p.ns.id == router.id && p.ns.interfaces.iter().any(|i| i.network_id == network_id)
    }) {
        return Err(PlanError::Invalid(format!(
            "router {} is listed twice on {network_id}",
            router.id
        )));
    }
    let rhex = device_hex(&router.id)?;
    let (gateway, cidr) = gateway_in_cidr(&router.gateway, router.prefix, &router.cidr)?;
    let dhcp = canonical_dhcp(&router.dhcp, &cidr)?;
    let external_ip = canonical_external_ip(&router.external_ip)?;
    let dnat = canonical_dnat(&router.dnat)?;
    debug_assert_eq!(net_hex.len(), 12);
    debug_assert_eq!(rhex.len(), 12);
    let veth_key = format!("{}{}", &net_hex[..6], &rhex[..6]);
    let iface = RouterInterface {
        network_id: network_id.to_string(),
        veth_bridge: claim(names, format!("a{veth_key}"), "a")?,
        veth_ns: claim(names, format!("b{veth_key}"), "b")?,
        gateway,
        prefix: router.prefix,
        cidr,
        dhcp,
    };

    if let Some(existing) = pending.iter_mut().find(|p| p.ns.id == router.id) {
        merge_router(&mut existing.ns, router, external_ip, dnat)?;
        existing.ns.interfaces.push(iface);
        return Ok(());
    }

    let snat = external_ip.is_some();
    pending.push(PendingRouter {
        at,
        ns: RouterNamespace {
            id: router.id.clone(),
            netns: claim(names, format!("ns{rhex}"), "ns")?,
            interfaces: vec![iface],
            external_veth_root: claim(names, format!("c{rhex}"), "c")?,
            external_veth_ns: claim(names, format!("d{rhex}"), "d")?,
            external_ip,
            external_dev: router.external_dev.clone(),
            snat,
            dnat,
            metadata: router.metadata,
        },
    });
    Ok(())
}

fn merge_router(
    ns: &mut RouterNamespace,
    router: &PlanRouter,
    external_ip: Option<String>,
    dnat: Vec<DnatRule>,
) -> Result<(), PlanError> {
    if let Some(ip) = external_ip {
        if let Some(have) = &ns.external_ip {
            if have != &ip {
                return Err(PlanError::Invalid(format!(
                    "router {} external ip disagrees",
                    router.id
                )));
            }
        } else {
            ns.external_ip = Some(ip);
        }
    }
    if !router.external_dev.is_empty() && ns.external_dev.is_empty() {
        ns.external_dev.clone_from(&router.external_dev);
    } else if !router.external_dev.is_empty() && ns.external_dev != router.external_dev {
        return Err(PlanError::Invalid(format!(
            "router {} external dev disagrees",
            router.id
        )));
    }
    if ns.metadata != router.metadata {
        return Err(PlanError::Invalid(format!(
            "router {} metadata flag disagrees",
            router.id
        )));
    }
    for rule in dnat {
        if let Some(prev) = ns.dnat.iter().find(|d| d.id == rule.id) {
            if prev != &rule {
                return Err(PlanError::Invalid(format!(
                    "dnat {} disagrees across router attachments",
                    rule.id
                )));
            }
            continue;
        }
        if ns.dnat.iter().any(|d| d.floating == rule.floating) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating address {}",
                rule.floating
            )));
        }
        ns.dnat.push(rule);
    }
    ns.snat = ns.external_ip.is_some();
    Ok(())
}

fn external_ips(networks: &[PlanNetwork]) -> Result<BTreeSet<String>, PlanError> {
    let mut out = BTreeSet::new();
    for net in networks {
        for router in &net.routers {
            if let Some(ip) = canonical_external_ip(&router.external_ip)? {
                out.insert(ip);
            }
        }
    }
    Ok(out)
}

fn canonical_provider(
    spec: &PlanProvider,
    hex: &str,
    names: &mut BTreeSet<String>,
) -> Result<ProviderAttach, PlanError> {
    let physnet = spec.physnet.as_str();
    if physnet.is_empty()
        || physnet.chars().any(char::is_whitespace)
        || physnet.contains('/')
        || physnet == "pistack0"
        || physnet == "wg-fleet"
        || physnet == "simbr0"
        || physnet.starts_with("pi")
    {
        return Err(PlanError::Invalid(format!("refusing physnet {physnet}")));
    }
    if let Some(vlan) = spec.vlan {
        if !(1..=4094).contains(&vlan) {
            return Err(PlanError::Invalid(format!(
                "vlan {vlan} is outside 1..=4094"
            )));
        }
    }
    Ok(ProviderAttach {
        physnet: physnet.to_string(),
        vlan: spec.vlan,
        link: claim(names, format!("pv{hex}"), "pv")?,
    })
}

fn plan_floating(
    net: &PlanNetwork,
    ports: &[PortEndpoint],
    external_ips: &BTreeSet<String>,
    seen_ids: &mut BTreeSet<String>,
    seen_addrs: &mut BTreeSet<String>,
    names: &mut BTreeSet<String>,
) -> Result<Vec<FloatingLocal>, PlanError> {
    let port_addrs: BTreeSet<&str> = ports.iter().map(|p| p.address.as_str()).collect();
    let mut out = Vec::with_capacity(net.floating.len());
    for float in &net.floating {
        if float.id.is_empty() {
            return Err(PlanError::Invalid("floating id is empty".into()));
        }
        if !seen_ids.insert(float.id.clone()) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating id {}",
                float.id
            )));
        }
        let floating = canonical_ip("floating", &float.floating)?.to_string();
        let fixed = canonical_ip("fixed", &float.fixed)?.to_string();
        if external_ips.contains(&floating) {
            return Err(PlanError::Invalid(format!(
                "floating address {floating} matches a router external ip"
            )));
        }
        if !seen_addrs.insert(floating.clone()) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating address {floating}"
            )));
        }
        if !port_addrs.contains(fixed.as_str()) {
            return Err(PlanError::Invalid(format!(
                "fixed address {fixed} is not a port on {}",
                net.id
            )));
        }
        let hex = device_hex(&float.id)?;
        out.push(FloatingLocal {
            id: float.id.clone(),
            floating,
            fixed,
            netns: claim(names, format!("nf{hex}"), "nf")?,
            veth_bridge: claim(names, format!("e{hex}"), "e")?,
            veth_ns: claim(names, format!("f{hex}"), "f")?,
        });
    }
    Ok(out)
}

fn canonical_dhcp(leases: &[DhcpLease], cidr: &str) -> Result<Vec<DhcpLease>, PlanError> {
    let net: IpNet = cidr.parse().map_err(|_| {
        PlanError::Invalid(format!("cidr {cidr} is not an ip network"))
    })?;
    let mut seen_mac = BTreeSet::new();
    let mut seen_addr = BTreeSet::new();
    let mut out = Vec::with_capacity(leases.len());
    for lease in leases {
        let mac = canonical_mac(&lease.mac)?;
        if !seen_mac.insert(mac.clone()) {
            return Err(PlanError::Invalid(format!("duplicate dhcp mac {mac}")));
        }
        let address = canonical_ip("dhcp address", &lease.address)?;
        if !net.contains(&address) {
            return Err(PlanError::Invalid(format!(
                "dhcp address {address} is outside {cidr}"
            )));
        }
        if !seen_addr.insert(address) {
            return Err(PlanError::Invalid(format!(
                "duplicate dhcp address {address}"
            )));
        }
        let mut dns = Vec::with_capacity(lease.dns.len());
        for entry in &lease.dns {
            dns.push(canonical_ip("dns", entry)?.to_string());
        }
        let mut routes = Vec::with_capacity(lease.routes.len());
        for route in &lease.routes {
            routes.push(DhcpRoute {
                destination: canonical_cidr("route destination", &route.destination)?,
                nexthop: canonical_ip("route nexthop", &route.nexthop)?.to_string(),
            });
        }
        out.push(DhcpLease {
            mac,
            address: address.to_string(),
            dns,
            routes,
        });
    }
    Ok(out)
}

fn overlay_mtu(network_mtu: u16, underlay_mtu: u16) -> Result<u16, PlanError> {
    let mtu = network_mtu.min(underlay_mtu.saturating_sub(50));
    if mtu == 0 {
        Err(PlanError::Mtu)
    } else {
        Ok(mtu)
    }
}

/// Unicast IPv4. Private, shared (100.64/10), public, and loopback are all
/// accepted. The agent, not the planner, refuses a public NIC.
fn usable(ip: Ipv4Addr) -> bool {
    let a = ip.octets()[0];
    !ip.is_unspecified() && !ip.is_broadcast() && !ip.is_multicast() && a != 0 && a < 240
}

fn unicast(role: &str, raw: &str) -> Result<Ipv4Addr, PlanError> {
    if raw.is_empty() || raw.contains('/') || raw.contains('%') {
        return Err(ip_err(role, raw));
    }
    match raw.parse::<Ipv4Addr>() {
        Ok(ip) if usable(ip) => Ok(ip),
        _ => Err(ip_err(role, raw)),
    }
}

fn ip_err(role: &str, raw: &str) -> PlanError {
    PlanError::Ip(format!("{role} {raw} is not a usable ipv4 address"))
}

/// Guest, gateway, floating, fixed, DHCP, DNS, and route next hops.
/// Display of the parsed address is the canonical form (no leading zeros,
/// compressed IPv6).
fn canonical_ip(role: &str, raw: &str) -> Result<IpAddr, PlanError> {
    if raw.is_empty()
        || raw.contains('/')
        || raw.contains('%')
        || raw.chars().any(char::is_whitespace)
    {
        return Err(PlanError::Ip(format!(
            "{role} {raw} is not a usable ip address"
        )));
    }
    match raw.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) if usable(ip) => Ok(IpAddr::V4(ip)),
        Ok(IpAddr::V6(ip)) if usable_v6(ip) => Ok(IpAddr::V6(ip)),
        _ => Err(PlanError::Ip(format!(
            "{role} {raw} is not a usable ip address"
        ))),
    }
}

fn usable_v6(ip: Ipv6Addr) -> bool {
    !ip.is_unspecified() && !ip.is_multicast()
}

/// Unique-local IPv6 (`fc00::/7`). Not link-local, not global, and not an underlay.
pub fn is_private_ula(ip: Ipv6Addr) -> bool {
    let [first, ..] = ip.octets();
    first & 0xfe == 0xfc
}

/// IPv6 SNAT is the external address plus every tenant prefix of that family.
/// A public IPv6 external address never reaches this check.
fn v6_snat_agrees(ns: &RouterNamespace) -> Result<(), PlanError> {
    let Some(ext) = &ns.external_ip else {
        return Ok(());
    };
    if !ext.contains(':') {
        return Ok(());
    }
    if !ns.snat {
        return Err(PlanError::Invalid(format!(
            "router {} has an ipv6 external address and snat is off",
            ns.id
        )));
    }
    for iface in &ns.interfaces {
        if !iface.cidr.contains(':') {
            return Err(PlanError::Invalid(format!(
                "router {} snat family does not match external ip",
                ns.id
            )));
        }
    }
    for rule in &ns.dnat {
        if !rule.floating.contains(':') {
            return Err(PlanError::Invalid(format!(
                "router {} floating family does not match external ip",
                ns.id
            )));
        }
    }
    Ok(())
}

fn canonical_cidr(role: &str, raw: &str) -> Result<String, PlanError> {
    let net: IpNet = raw.parse().map_err(|_| {
        PlanError::Invalid(format!("{role} {raw} is not an ip network"))
    })?;
    let canonical = net.trunc().to_string();
    if canonical != raw {
        return Err(PlanError::Invalid(format!(
            "{role} {raw} is not canonical"
        )));
    }
    Ok(canonical)
}

fn canonical_mac(raw: &str) -> Result<String, PlanError> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 6 {
        return Err(PlanError::Mac(raw.to_string()));
    }
    let mut out = String::with_capacity(17);
    for (i, part) in parts.iter().enumerate() {
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(PlanError::Mac(raw.to_string()));
        }
        if i > 0 {
            out.push(':');
        }
        out.push_str(&part.to_ascii_lowercase());
    }
    Ok(out)
}

fn gateway_in_cidr(gateway: &str, prefix: u8, cidr: &str) -> Result<(String, String), PlanError> {
    match canonical_ip("gateway", gateway)? {
        IpAddr::V4(gw) => {
            if !(1..=30).contains(&prefix) {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} is outside 1..=30"
                )));
            }
            let net: Ipv4Net = cidr.parse().map_err(|_| {
                PlanError::Invalid(format!("cidr {cidr} is not an ipv4 network"))
            })?;
            if net.prefix_len() != prefix {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} does not match cidr {cidr}"
                )));
            }
            if !net.contains(&gw) || gw == net.network() || gw == net.broadcast() {
                return Err(PlanError::Invalid(format!(
                    "gateway {gateway} is outside {cidr}"
                )));
            }
            Ok((gw.to_string(), net.trunc().to_string()))
        }
        IpAddr::V6(gw) => {
            if !(1..=128).contains(&prefix) {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} is outside 1..=128"
                )));
            }
            let net: Ipv6Net = cidr.parse().map_err(|_| {
                PlanError::Invalid(format!("cidr {cidr} is not an ipv6 network"))
            })?;
            if net.prefix_len() != prefix {
                return Err(PlanError::Invalid(format!(
                    "prefix {prefix} does not match cidr {cidr}"
                )));
            }
            if !net.contains(&gw) {
                return Err(PlanError::Invalid(format!(
                    "gateway {gateway} is outside {cidr}"
                )));
            }
            Ok((gw.to_string(), net.trunc().to_string()))
        }
    }
}

fn canonical_external_ip(raw: &Option<String>) -> Result<Option<String>, PlanError> {
    match raw {
        None => Ok(None),
        Some(s) => match canonical_ip("external ip", s)? {
            IpAddr::V4(ip) => Ok(Some(ip.to_string())),
            IpAddr::V6(ip) if is_private_ula(ip) => Ok(Some(ip.to_string())),
            IpAddr::V6(ip) => Err(PlanError::Ip(format!(
                "external ip {ip} is not a private ipv6 address"
            ))),
        },
    }
}

fn canonical_dnat(rules: &[DnatRule]) -> Result<Vec<DnatRule>, PlanError> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        if rule.id.is_empty() {
            return Err(PlanError::Invalid("dnat rule has an empty id".into()));
        }
        let floating = canonical_ip("floating", &rule.floating)?.to_string();
        let fixed = canonical_ip("fixed", &rule.fixed)?.to_string();
        if out.iter().any(|d: &DnatRule| d.id == rule.id) {
            return Err(PlanError::Invalid(format!("duplicate dnat {}", rule.id)));
        }
        if out.iter().any(|d: &DnatRule| d.floating == floating) {
            return Err(PlanError::Invalid(format!(
                "duplicate floating address {floating}"
            )));
        }
        out.push(DnatRule {
            id: rule.id.clone(),
            floating,
            fixed,
        });
    }
    Ok(out)
}

fn canonical_rules(rules: &[SgRule]) -> Result<Vec<SgRule>, PlanError> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        if !matches!(rule.proto.as_str(), "tcp" | "udp" | "icmp" | "any") {
            return Err(PlanError::Invalid(format!(
                "security group proto {} is not tcp, udp, icmp, or any",
                rule.proto
            )));
        }
        if rule.port_min > rule.port_max {
            return Err(PlanError::Invalid(format!(
                "security group port {} > {}",
                rule.port_min, rule.port_max
            )));
        }
        let cidr = canonical_cidr("security group cidr", &rule.cidr)?;
        out.push(SgRule {
            proto: rule.proto.clone(),
            port_min: rule.port_min,
            port_max: rule.port_max,
            cidr,
        });
    }
    Ok(out)
}

/// Host-NAT bridges are `pistack0` and `pi` plus hex. Overlay names use another prefix.
fn claim(names: &mut BTreeSet<String>, name: String, prefix: &str) -> Result<String, PlanError> {
    if name == "pistack0"
        || name.starts_with("pi")
        || name.len() > 15
        || !is_hex_suffix(&name, prefix)
    {
        return Err(PlanError::Name(name));
    }
    if !names.insert(name.clone()) {
        return Err(PlanError::Invalid(format!("duplicate device name {name}")));
    }
    Ok(name)
}

fn is_hex_suffix(name: &str, prefix: &str) -> bool {
    match name.strip_prefix(prefix) {
        Some(rest) => {
            rest.len() == 12
                && rest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_A: &str = "net-01a10318-a507-74fd-a86e-5acbda974722";
    const NET_B: &str = "net-01a20318-b607-74fd-a86e-5acbda974733";
    const RTR_A: &str = "rtr-01a30318-c507-74fd-a86e-5acbda974744";
    const RTR_B: &str = "rtr-01a40318-d607-74fd-a86e-5acbda974755";

    fn host(ip: &str) -> PlanHost {
        PlanHost {
            underlay: ip.into(),
            underlay_mtu: 0,
        }
    }

    fn network(id: &str, vni: u32, routers: Vec<PlanRouter>, ports: Vec<PlanPort>) -> PlanNetwork {
        PlanNetwork {
            id: id.into(),
            vni,
            mtu: 1500,
            ports,
            routers,
            provider: None,
            floating: vec![],
        }
    }

    fn router(id: &str, gateway: &str, cidr: &str) -> PlanRouter {
        PlanRouter {
            id: id.into(),
            gateway: gateway.into(),
            prefix: 24,
            cidr: cidr.into(),
            external_ip: None,
            external_dev: String::new(),
            dnat: Vec::new(),
            metadata: false,
            dhcp: vec![],
        }
    }

    fn port(id: &str, mac: &str, underlay: &str, address: &str) -> PlanPort {
        PlanPort {
            id: id.into(),
            mac: mac.into(),
            address: address.into(),
            underlay: underlay.into(),
            ingress: Vec::new(),
            egress: Vec::new(),
        }
    }

    #[test]
    fn two_networks_get_distinct_vnis_and_devices() {
        assert_eq!(VXLAN_PORT, 4789);
        let nets = [
            network(NET_A, 1, vec![], vec![]),
            network(NET_B, 2, vec![], vec![]),
        ];
        let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        assert_eq!(plan.version, PLAN_VERSION);
        assert_eq!(plan.host, "10.0.0.1");
        assert_eq!(plan.networks.len(), 2);
        assert_ne!(plan.networks[0].vni, plan.networks[1].vni);
        assert_eq!(plan.networks[0].vni, 1);
        assert_eq!(plan.networks[1].vni, 2);
        assert_eq!(plan.networks[0].bridge, "po01a10318a507");
        assert_eq!(plan.networks[0].vxlan, "vx01a10318a507");
        assert_eq!(plan.networks[1].bridge, "po01a20318b607");
        assert_eq!(plan.networks[1].vxlan, "vx01a20318b607");
        assert_ne!(plan.networks[0].bridge, plan.networks[1].bridge);
        assert_ne!(plan.networks[0].vxlan, plan.networks[1].vxlan);
        assert_eq!(plan.networks[0].mtu, 1450);
        assert_eq!(plan.networks[1].mtu, 1450);
    }

    #[test]
    fn same_cidr_is_two_bridges_and_two_namespaces() {
        let nets = [
            network(
                NET_A,
                7,
                vec![router(RTR_A, "10.90.0.1", "10.90.0.0/24")],
                vec![],
            ),
            network(
                NET_B,
                8,
                vec![router(RTR_B, "10.90.0.1", "10.90.0.0/24")],
                vec![],
            ),
        ];
        let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        assert_eq!(plan.networks.len(), 2);
        assert_ne!(plan.networks[0].vni, plan.networks[1].vni);
        assert_ne!(plan.networks[0].bridge, plan.networks[1].bridge);
        assert_ne!(plan.networks[0].vxlan, plan.networks[1].vxlan);
        let a = &plan.networks[0].routers[0];
        let b = &plan.networks[1].routers[0];
        assert_ne!(a.netns, b.netns);
        assert_eq!(a.interfaces[0].gateway, "10.90.0.1");
        assert_eq!(b.interfaces[0].gateway, "10.90.0.1");
        assert_eq!(a.interfaces[0].cidr, "10.90.0.0/24");
        assert_eq!(b.interfaces[0].cidr, "10.90.0.0/24");
        assert!(!a.snat);
        assert!(!b.snat);
    }

    #[test]
    fn peers_are_remote_underlays_only() {
        let ports = vec![
            port(
                "port-01a50318-a507-74fd-a86e-5acbda974701",
                "52:54:00:00:00:22",
                "10.0.0.2",
                "10.90.0.22",
            ),
            port(
                "port-01a50318-a507-74fd-a86e-5acbda974702",
                "52:54:00:00:00:11",
                "10.0.0.1",
                "10.90.0.11",
            ),
        ];
        let nets = [network(NET_A, 1, vec![], ports)];
        let peers = vec![
            "10.0.0.10".to_string(),
            "10.0.0.2".to_string(),
            "10.0.0.2".to_string(),
        ];
        let plan = plan(&host("10.0.0.1"), &nets, &peers).unwrap();
        assert_eq!(
            plan.networks[0].peers,
            vec!["10.0.0.2".to_string(), "10.0.0.10".to_string()]
        );
        assert!(!plan.networks[0].peers.iter().any(|p| p == "0.0.0.0"));
        assert!(!plan.networks[0].peers.iter().any(|p| p == "10.0.0.1"));
        assert_eq!(plan.networks[0].ports[0].underlay, "10.0.0.2");
        assert_eq!(plan.networks[0].ports[0].mac, "52:54:00:00:00:22");
        assert_eq!(plan.networks[0].ports[1].underlay, "10.0.0.1");
        assert_eq!(plan.networks[0].ports[1].mac, "52:54:00:00:00:11");
        assert_eq!(plan.networks[0].local, "10.0.0.1");
    }

    #[test]
    fn unspecified_peer_is_an_error() {
        let nets = [network(NET_A, 1, vec![], vec![])];
        let err = plan(&host("10.0.0.1"), &nets, &["0.0.0.0".into()]).unwrap_err();
        assert!(matches!(err, PlanError::Ip(ref msg) if msg.contains("0.0.0.0")));
        for bad in [
            "255.255.255.255",
            "224.0.0.1",
            "239.255.255.255",
            "::1",
            "not-an-ip",
        ] {
            let nets = [network(NET_A, 1, vec![], vec![])];
            assert!(
                plan(&host("10.0.0.1"), &nets, &[bad.into()]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn planned_names_are_not_hostnat_bridges() {
        let nets = [
            network(
                NET_A,
                7,
                vec![router(RTR_A, "10.90.0.1", "10.90.0.0/24")],
                vec![],
            ),
            network(
                NET_B,
                8,
                vec![router(RTR_B, "10.91.0.1", "10.91.0.0/24")],
                vec![],
            ),
        ];
        let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        assert!(!plan.networks.is_empty());
        for seg in &plan.networks {
            assert!(
                is_bridge(&seg.bridge),
                "{} does not match ^po[0-9a-f]{{12}}$",
                seg.bridge
            );
            assert_device(&seg.bridge);
            assert_device(&seg.vxlan);
            for r in &seg.routers {
                assert_device(&r.netns);
                assert_device(&r.external_veth_root);
                assert_device(&r.external_veth_ns);
                for iface in &r.interfaces {
                    assert_device(&iface.veth_bridge);
                    assert_device(&iface.veth_ns);
                }
            }
        }
    }

    fn is_bridge(name: &str) -> bool {
        let b = name.as_bytes();
        b.len() == 14
            && b.starts_with(b"po")
            && b[2..]
                .iter()
                .all(|c| matches!(*c, b'0'..=b'9' | b'a'..=b'f'))
    }

    fn assert_device(name: &str) {
        assert_ne!(name, "pistack0", "{name}");
        assert!(!name.starts_with("pi"), "{name} starts with pi");
        assert!(name.len() <= 15, "{name}");
    }

    #[test]
    fn vni_bounds() {
        let zero = [network(NET_A, 0, vec![], vec![])];
        assert!(matches!(
            plan(&host("10.0.0.1"), &zero, &[]).unwrap_err(),
            PlanError::Vni(0)
        ));
        let big = [network(NET_A, 16_777_216, vec![], vec![])];
        assert!(matches!(
            plan(&host("10.0.0.1"), &big, &[]).unwrap_err(),
            PlanError::Vni(16_777_216)
        ));
        let max = [network(NET_A, 16_777_215, vec![], vec![])];
        assert_eq!(plan(&host("10.0.0.1"), &max, &[]).unwrap().networks[0].vni, 16_777_215);
    }

    #[test]
    fn device_hex_strips_kind_prefix() {
        assert_eq!(
            device_hex("net-01a10318-a507-74fd-a86e-5acbda974722").unwrap(),
            "01a10318a507"
        );
    }

    #[test]
    fn one_router_two_networks_is_one_namespace() {
        let mut leg_a = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
        leg_a.external_ip = Some("10.66.66.9".into());
        leg_a.external_dev = "wg-fleet".into();
        leg_a.metadata = true;
        leg_a.dnat = vec![DnatRule {
            id: "fip1".into(),
            floating: "10.66.66.80".into(),
            fixed: "10.90.0.5".into(),
        }];
        let mut leg_b = router(RTR_A, "10.91.0.1", "10.91.0.0/24");
        leg_b.external_ip = Some("10.66.66.9".into());
        leg_b.external_dev = "wg-fleet".into();
        leg_b.metadata = true;
        let nets = [
            network(NET_A, 3, vec![leg_a], vec![]),
            network(NET_B, 4, vec![leg_b], vec![]),
        ];
        let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        let nss: Vec<_> = plan.networks.iter().flat_map(|s| &s.routers).collect();
        assert_eq!(nss.len(), 1);
        let ns = nss[0];
        assert_eq!(ns.interfaces.len(), 2);
        assert_eq!(ns.interfaces[0].network_id, NET_A);
        assert_eq!(ns.interfaces[1].network_id, NET_B);
        assert_eq!(ns.interfaces[0].gateway, "10.90.0.1");
        assert_eq!(ns.interfaces[1].gateway, "10.91.0.1");
        let hex = device_hex(RTR_A).unwrap();
        let a = device_hex(NET_A).unwrap();
        let b = device_hex(NET_B).unwrap();
        assert_eq!(ns.netns, format!("ns{hex}"));
        assert_eq!(ns.external_veth_root, format!("c{hex}"));
        assert_eq!(ns.external_veth_ns, format!("d{hex}"));
        assert_eq!(
            ns.interfaces[0].veth_bridge,
            format!("a{}{}", &a[..6], &hex[..6])
        );
        assert_eq!(
            ns.interfaces[0].veth_ns,
            format!("b{}{}", &a[..6], &hex[..6])
        );
        assert_eq!(
            ns.interfaces[1].veth_bridge,
            format!("a{}{}", &b[..6], &hex[..6])
        );
        assert!(ns.snat);
        assert!(ns.metadata);
        assert_eq!(ns.external_dev, "wg-fleet");
        assert_eq!(ns.external_ip.as_deref(), Some("10.66.66.9"));
        assert_eq!(ns.dnat.len(), 1);
        assert_eq!(ns.dnat[0].floating, "10.66.66.80");
        assert_eq!(ns.dnat[0].fixed, "10.90.0.5");
    }

    #[test]
    fn plan_json_round_trips() {
        let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
        leg.external_ip = Some("10.66.66.9".into());
        leg.external_dev = "wg-fleet".into();
        leg.metadata = true;
        leg.dnat = vec![DnatRule {
            id: "fip1".into(),
            floating: "10.66.66.80".into(),
            fixed: "10.90.0.5".into(),
        }];
        let mut guest = port(
            "port-01a50318-a507-74fd-a86e-5acbda974701",
            "52:54:00:00:00:22",
            "10.0.0.2",
            "10.90.0.22",
        );
        guest.ingress = vec![SgRule {
            proto: "tcp".into(),
            port_min: 22,
            port_max: 22,
            cidr: "10.66.66.0/24".into(),
        }];
        guest.egress = vec![SgRule {
            proto: "any".into(),
            port_min: 0,
            port_max: 0,
            cidr: "0.0.0.0/0".into(),
        }];
        let nets = [network(NET_A, 9, vec![leg], vec![guest])];
        let plan = plan(&host("10.0.0.1"), &nets, &["10.0.0.2".into()]).unwrap();
        let json = serde_json::to_string(&plan).unwrap();
        let back: OverlayPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
        assert_eq!(back.version, 1);
        assert_eq!(back.networks[0].ports[0].ingress[0].proto, "tcp");
        assert!(back.networks[0].routers[0].snat);
        assert!(back.networks[0].provider.is_none());
        assert!(back.networks[0].floating.is_empty());
        assert!(back.networks[0].routers[0].interfaces[0].dhcp.is_empty());
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let net = value["networks"][0].as_object_mut().unwrap();
        net.remove("provider");
        net.remove("floating");
        net["routers"][0]["interfaces"][0]
            .as_object_mut()
            .unwrap()
            .remove("dhcp");
        let old: OverlayPlan = serde_json::from_value(value).unwrap();
        assert!(old.networks[0].provider.is_none());
        assert!(old.networks[0].floating.is_empty());
        assert!(old.networks[0].routers[0].interfaces[0].dhcp.is_empty());
    }

    #[test]
    fn mtu_is_underlay_minus_vxlan_overhead() {
        let mut n = network(NET_A, 1, vec![], vec![]);
        let mut h = host("10.0.0.1");
        h.underlay_mtu = 0;
        assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1450);
        h.underlay_mtu = 1500;
        assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1450);
        h.underlay_mtu = 9000;
        assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1500);
        n.mtu = 1400;
        h.underlay_mtu = 1500;
        assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1400);
        n.mtu = 0;
        assert!(matches!(
            plan(&h, &[n], &[]).unwrap_err(),
            PlanError::Mtu
        ));
    }

    #[test]
    fn loopback_and_public_underlay_are_usable() {
        let nets = [network(NET_A, 1, vec![], vec![])];
        assert!(plan(&host("127.0.0.1"), &nets, &[]).is_ok());
        let nets = [network(NET_A, 1, vec![], vec![])];
        assert!(plan(&host("8.8.8.8"), &nets, &["100.64.0.1".into()]).is_ok());
        let nets = [network(NET_A, 1, vec![], vec![])];
        assert!(plan(&host("0.0.0.0"), &nets, &[]).is_err());
        let ports = vec![port(
            "port-01a50318-a507-74fd-a86e-5acbda974701",
            "52:54:00:00:00:11",
            "",
            "10.90.0.11",
        )];
        let nets = [network(NET_A, 1, vec![], ports)];
        let err = plan(&host("10.0.0.1"), &nets, &[]).unwrap_err();
        assert!(matches!(err, PlanError::Invalid(ref msg) if msg.contains("empty underlay")));
    }

    #[test]
    fn provider_network_skips_vxlan_and_peers() {
        let mut provider = network(NET_A, 0, vec![], vec![]);
        provider.provider = Some(PlanProvider {
            physnet: "eno1".into(),
            vlan: Some(20),
        });
        let overlay = network(NET_B, 2, vec![], vec![]);
        let plan = plan(
            &host("10.0.0.1"),
            &[provider, overlay],
            &["10.0.0.2".into()],
        )
        .unwrap();
        let seg = &plan.networks[0];
        assert_eq!(seg.vni, 0);
        assert_eq!(seg.bridge, "po01a10318a507");
        assert_eq!(seg.vxlan, "");
        let attach = seg.provider.as_ref().unwrap();
        assert_eq!(attach.physnet, "eno1");
        assert_eq!(attach.vlan, Some(20));
        assert!(attach.link.starts_with("pv"), "{}", attach.link);
        assert_eq!(attach.link, "pv01a10318a507");
        assert!(seg.peers.is_empty());
        assert!(!seg.peers.iter().any(|p| p == "0.0.0.0"));
        assert_device(&attach.link);
        assert_eq!(plan.networks[1].vxlan, "vx01a20318b607");
        assert_eq!(plan.networks[1].peers, vec!["10.0.0.2".to_string()]);
        assert!(plan.networks[1].provider.is_none());
    }

    #[test]
    fn dhcp_lease_round_trips_onto_the_interface() {
        let guest = port(
            "port-01a50318-a507-74fd-a86e-5acbda974701",
            "52:54:00:00:00:22",
            "10.0.0.1",
            "10.90.0.22",
        );
        let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
        leg.dhcp = vec![DhcpLease {
            mac: "52:54:00:00:00:22".into(),
            address: "10.90.0.22".into(),
            dns: vec!["1.1.1.1".into()],
            routes: vec![DhcpRoute {
                destination: "10.8.0.9/32".into(),
                nexthop: "10.90.0.1".into(),
            }],
        }];
        let nets = [network(NET_A, 9, vec![leg], vec![guest])];
        let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        let dhcp = &plan.networks[0].routers[0].interfaces[0].dhcp;
        assert_eq!(dhcp.len(), 1);
        assert_eq!(dhcp[0].mac, "52:54:00:00:00:22");
        assert_eq!(dhcp[0].address, "10.90.0.22");
        assert_eq!(dhcp[0].dns, vec!["1.1.1.1".to_string()]);
        assert_eq!(dhcp[0].routes.len(), 1);
        assert_eq!(dhcp[0].routes[0].destination, "10.8.0.9/32");
        assert_eq!(dhcp[0].routes[0].nexthop, "10.90.0.1");
        let json = serde_json::to_string(&plan).unwrap();
        let back: OverlayPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
        assert!(json.contains("10.8.0.9/32"));
        assert!(json.contains("1.1.1.1"));
    }

    #[test]
    fn floating_fixed_on_this_segment_is_local() {
        let guest = port(
            "port-01a50318-a507-74fd-a86e-5acbda974701",
            "52:54:00:00:00:11",
            "10.0.0.1",
            "10.90.0.11",
        );
        let leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
        let mut net = network(NET_A, 1, vec![leg], vec![guest]);
        let id = "fip-01a60318-e507-74fd-a86e-5acbda974766";
        net.floating = vec![PlanFloat {
            id: id.into(),
            floating: "10.66.66.80".into(),
            fixed: "10.90.0.11".into(),
        }];
        let plan = plan(&host("10.0.0.1"), &[net], &[]).unwrap();
        assert!(plan.networks[0].routers[0].dnat.is_empty());
        let float = &plan.networks[0].floating[0];
        let hex = device_hex(id).unwrap();
        assert_eq!(float.id, id);
        assert_eq!(float.floating, "10.66.66.80");
        assert_eq!(float.fixed, "10.90.0.11");
        assert_eq!(float.netns, format!("nf{hex}"));
        assert_eq!(float.veth_bridge, format!("e{hex}"));
        assert_eq!(float.veth_ns, format!("f{hex}"));
        assert!(float.netns.starts_with("nf"));
        assert!(float.veth_bridge.starts_with('e'));
        assert!(float.veth_ns.starts_with('f'));
        assert_device(&float.netns);
        assert_device(&float.veth_bridge);
        assert_device(&float.veth_ns);
    }

    #[test]
    fn ipv6_port_and_gateway_are_canonical() {
        let mut guest = port(
            "port-01a50318-a507-74fd-a86e-5acbda974701",
            "52:54:00:00:00:11",
            "10.0.0.1",
            "2001:0db8:0000:0000:0000:0000:0000:0010",
        );
        guest.ingress = vec![SgRule {
            proto: "tcp".into(),
            port_min: 22,
            port_max: 22,
            cidr: "2001:db8::/64".into(),
        }];
        let mut leg = router(
            RTR_A,
            "2001:0db8:0000:0001:0000:0000:0000:0001",
            "2001:0db8:0000:0001::/64",
        );
        leg.prefix = 64;
        let nets = [
            network(NET_A, 1, vec![], vec![guest]),
            network(NET_B, 2, vec![leg], vec![]),
        ];
        let got = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        assert_eq!(got.networks[0].ports[0].address, "2001:db8::10");
        assert_eq!(got.networks[0].ports[0].ingress[0].cidr, "2001:db8::/64");
        let iface = &got.networks[1].routers[0].interfaces[0];
        assert_eq!(iface.gateway, "2001:db8:0:1::1");
        assert_eq!(iface.prefix, 64);
        assert_eq!(iface.cidr, "2001:db8:0:1::/64");
        assert_eq!(got.host, "10.0.0.1");

        let mut bad = port(
            "port-01a50318-a507-74fd-a86e-5acbda974702",
            "52:54:00:00:00:12",
            "10.0.0.1",
            "10.90.0.12",
        );
        bad.egress = vec![SgRule {
            proto: "any".into(),
            port_min: 0,
            port_max: 0,
            cidr: "2001:0db8::/64".into(),
        }];
        let nets = [network(NET_A, 1, vec![], vec![bad])];
        assert!(plan(&host("10.0.0.1"), &nets, &[]).is_err());
    }

    #[test]
    fn ipv6_external_snat_is_a_private_ula() {
        let mut leg = router(RTR_A, "fd00:0090::0001", "fd00:0090::/64");
        leg.prefix = 64;
        leg.external_ip = Some("fd00:0066::0009".into());
        leg.external_dev = "external".into();
        leg.dnat = vec![DnatRule {
            id: "fip6".into(),
            floating: "fd00:0066::000a".into(),
            fixed: "fd00:0090::0010".into(),
        }];
        let nets = [network(NET_A, 11, vec![leg], vec![])];
        let got = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        let ns = &got.networks[0].routers[0];
        assert!(ns.snat);
        assert_eq!(ns.external_ip.as_deref(), Some("fd00:66::9"));
        assert_eq!(ns.interfaces[0].cidr, "fd00:90::/64");
        assert_eq!(ns.interfaces[0].gateway, "fd00:90::1");
        assert_eq!(ns.dnat[0].floating, "fd00:66::a");
        assert_eq!(ns.dnat[0].fixed, "fd00:90::10");
        assert_eq!(got.host, "10.0.0.1");

        let mut ula = router(RTR_A, "fc00::1", "fc00::/64");
        ula.prefix = 64;
        ula.external_ip = Some("fc00:66::1".into());
        let nets = [network(NET_A, 12, vec![ula], vec![])];
        let got = plan(&host("10.1.0.1"), &nets, &[]).unwrap();
        assert!(got.networks[0].routers[0].snat);
        assert_eq!(
            got.networks[0].routers[0].external_ip.as_deref(),
            Some("fc00:66::1")
        );

        for bad in ["2001:db8::9", "fe80::1", "::1", "2001:db8:1::1"] {
            let mut leg = router(RTR_A, "fd00:90::1", "fd00:90::/64");
            leg.prefix = 64;
            leg.external_ip = Some(bad.into());
            let nets = [network(NET_A, 13, vec![leg], vec![])];
            let err = plan(&host("10.0.0.1"), &nets, &[]).unwrap_err();
            assert!(
                matches!(err, PlanError::Ip(ref msg) if msg.contains("not a private ipv6")),
                "{bad}: {err}"
            );
        }

        // An IPv6 address is not an underlay, public or unique-local.
        let nets = [network(NET_A, 1, vec![], vec![])];
        assert!(plan(&host("2001:db8::1"), &nets, &[]).is_err());
        let nets = [network(NET_A, 1, vec![], vec![])];
        assert!(plan(&host("fd00::1"), &nets, &[]).is_err());
        let nets = [network(NET_A, 1, vec![], vec![])];
        assert!(plan(&host("10.0.0.1"), &nets, &["2001:db8::2".into()]).is_err());

        let mut mixed = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
        mixed.external_ip = Some("fd00:66::9".into());
        let nets = [network(NET_A, 14, vec![mixed], vec![])];
        let err = plan(&host("10.0.0.1"), &nets, &[]).unwrap_err();
        assert!(matches!(err, PlanError::Invalid(ref msg) if msg.contains("snat family")));
    }

    #[test]
    fn router_without_external_ip_sends_no_external_dev() {
        let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
        leg.external_dev = "external".into();
        let nets = [network(NET_A, 3, vec![leg], vec![])];
        let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
        let ns = &plan.networks[0].routers[0];
        assert!(ns.external_ip.is_none());
        assert!(!ns.snat);
        assert!(ns.external_dev.is_empty());
        let value = serde_json::to_value(&plan).unwrap();
        let doc = &value["networks"][0]["routers"][0];
        assert!(doc.get("external_dev").is_none(), "{doc}");
        let back: OverlayPlan = serde_json::from_value(value).unwrap();
        assert!(back.networks[0].routers[0].external_dev.is_empty());
    }

    #[test]
    fn simbr0_physnet_is_refused() {
        let mut provider = network(NET_A, 0, vec![], vec![]);
        provider.provider = Some(PlanProvider {
            physnet: "simbr0".into(),
            vlan: None,
        });
        assert!(plan(&host("10.0.0.1"), &[provider], &[]).is_err());
        let mut empty = network(NET_A, 0, vec![], vec![]);
        empty.provider = Some(PlanProvider {
            physnet: String::new(),
            vlan: None,
        });
        assert!(plan(&host("10.0.0.1"), &[empty], &[]).is_err());
    }
}
