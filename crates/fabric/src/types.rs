// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The network resource model: Network, Subnet, Port, Router,
//! SecurityGroup, FloatingIp.
//!
//! `spec` is the user's, `status` is the reconciler's. Every status carries
//! `observed_revision`, the revision the reconciler last acted on, so a
//! status write (which bumps the revision) does not trigger another round.

use ipnet::IpNet;
use pistack_types::{kind, Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use uuid::Uuid;

/// Define a resource struct with the `Resource<Spec, Status>` wire shape
/// (`meta` flattened, then `spec` and `status`). A named struct rather than
/// the alias because the orphan rule forbids `impl Kind for Resource<..>`
/// outside `pistack-types`; `From` both ways keeps them interchangeable.
macro_rules! resource {
    ($(#[$doc:meta])* $t:ident, $spec:ty, $status:ty, $k:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct $t {
            #[serde(flatten)]
            pub meta: Meta,
            pub spec: $spec,
            pub status: $status,
        }
        impl Kind for $t {
            const KIND: &'static str = $k;
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        impl From<Resource<$spec, $status>> for $t {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
        impl From<$t> for Resource<$spec, $status> {
            fn from(r: $t) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
    };
}

// ---- Network ---------------------------------------------------------------

/// How a network is realised on the wire.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NetworkKind {
    /// VXLAN overlay. The segment id is the VNI.
    #[default]
    Overlay,
    /// Mapped onto a physical network, optionally VLAN-tagged.
    Provider {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vlan: Option<u16>,
        physnet: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSpec {
    #[serde(default)]
    pub kind: NetworkKind,
    #[serde(default = "default_mtu")]
    pub mtu: u16,
    /// External networks are where routers get their gateway and floating
    /// IPs are drawn from.
    #[serde(default)]
    pub external: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_domain: Option<String>,
    /// Give every machine that has an IPv4 a floating IP from this network
    /// without being asked. Off unless set. Only an external network can
    /// carry it, and only one network in the cloud at a time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_floating_ip: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn default_mtu() -> u16 {
    1500
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self {
            kind: NetworkKind::Overlay,
            mtu: default_mtu(),
            external: false,
            dns_domain: None,
            auto_floating_ip: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NetworkStatus {
    #[serde(default)]
    pub phase: Phase,
    /// VNI for overlays, VLAN tag for provider networks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<u32>,
    /// `hostnat` or `overlay`. Set once. A network that already has a
    /// host-NAT port stays `hostnat`, so later guests join that bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datapath: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(Network, NetworkSpec, NetworkStatus, kind::NETWORK);

// ---- Subnet ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllocationPool {
    pub start: IpAddr,
    pub end: IpAddr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostRoute {
    pub destination: IpNet,
    pub nexthop: IpAddr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubnetSpec {
    pub network: Id,
    pub cidr: IpNet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<IpAddr>,
    /// Empty means "the whole CIDR minus network, broadcast and gateway".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allocation_pools: Vec<AllocationPool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_nameservers: Vec<IpAddr>,
    #[serde(default = "default_true")]
    pub enable_dhcp: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_routes: Vec<HostRoute>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SubnetStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Addresses currently held by ports and floating IPs.
    #[serde(default)]
    pub allocated: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(Subnet, SubnetSpec, SubnetStatus, kind::SUBNET);

// ---- Port ------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortSpec {
    pub network: Id,
    /// Chosen on create when absent: the network's first subnet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet: Option<Id>,
    /// Allocated on create when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_ips: Vec<IpAddr>,
    /// Generated on create when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mac: String,
    /// What the port is plugged into: a machine, or a router for gateway and
    /// interface ports. The id's kind says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_groups: Vec<Id>,
    #[serde(default = "default_true")]
    pub port_security: bool,
}

impl Default for PortSpec {
    fn default() -> Self {
        Self {
            network: Id::new(kind::NETWORK),
            subnet: None,
            fixed_ips: Vec::new(),
            mac: String::new(),
            device: None,
            security_groups: Vec::new(),
            port_security: true,
        }
    }
}

/// What the backend reports once a port is realised.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PortBinding {
    /// `ovs`, `mock`, ...
    pub vif_type: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vif_details: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PortStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Chassis the port is bound on, once the device is placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<PortBinding>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(Port, PortSpec, PortStatus, kind::PORT);

impl Port {
    /// True for ports fabric itself created for a router.
    pub fn is_router_port(&self) -> bool {
        self.spec
            .device
            .as_ref()
            .is_some_and(|d| d.kind == kind::ROUTER)
    }
}

// ---- Router ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StaticRoute {
    pub destination: IpNet,
    pub nexthop: IpAddr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RouterSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_network: Option<Id>,
    /// Subnets the router has an interface on. Managed with
    /// `PUT /routers/{id}/interfaces`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<StaticRoute>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RouterStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Port on the external network, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_port: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ip: Option<IpAddr>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(Router, RouterSpec, RouterStatus, kind::ROUTER);

// ---- SecurityGroup ---------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Ingress,
    Egress,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum EtherType {
    #[default]
    IPv4,
    IPv6,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    /// Any protocol.
    Any,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PortRange {
    pub min: u16,
    pub max: u16,
}

/// Who the rule matches on the far side. Exactly one of these.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Remote {
    /// Any address of the rule's ethertype.
    #[default]
    Any,
    Cidr(IpNet),
    /// Members of another (or the same) security group.
    Group(Id),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rule {
    /// Stable handle for `DELETE /security-groups/{id}/rules/{rule}`.
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    pub direction: Direction,
    #[serde(default)]
    pub ethertype: EtherType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_range: Option<PortRange>,
    #[serde(default)]
    pub remote: Remote,
}

impl Rule {
    pub fn new(direction: Direction, ethertype: EtherType) -> Self {
        Self {
            id: Uuid::now_v7(),
            direction,
            ethertype,
            protocol: None,
            port_range: None,
            remote: Remote::Any,
        }
    }
    pub fn protocol(mut self, p: Protocol) -> Self {
        self.protocol = Some(p);
        self
    }
    pub fn ports(mut self, min: u16, max: u16) -> Self {
        self.port_range = Some(PortRange { min, max });
        self
    }
    pub fn remote(mut self, r: Remote) -> Self {
        self.remote = r;
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SecurityGroupSpec {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SecurityGroupStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Ports currently members of the group.
    #[serde(default)]
    pub members: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    SecurityGroup,
    SecurityGroupSpec,
    SecurityGroupStatus,
    kind::SECGROUP
);

// ---- FloatingIp ------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FloatingIpSpec {
    /// External network the address is drawn from.
    pub network: Id,
    /// Allocated on create when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<IpAddr>,
    /// Port the address is NATed to, if associated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<Id>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FloatingIpStatus {
    #[serde(default)]
    pub phase: Phase,
    /// The port's fixed IP the floating address maps to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_ip: Option<IpAddr>,
    /// Router carrying the NAT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<Id>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    FloatingIp,
    FloatingIpSpec,
    FloatingIpStatus,
    kind::FLOATING_IP
);

/// Every kind fabric owns, for the reconciler's event filter.
pub const KINDS: &[&str] = &[
    kind::NETWORK,
    kind::SUBNET,
    kind::PORT,
    kind::ROUTER,
    kind::SECGROUP,
    kind::FLOATING_IP,
];
