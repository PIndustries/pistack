// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The network resource model: Vpc, Network, Subnet, Port, Router,
//! SecurityGroup, FloatingIp.
//!
//! `spec` is the user's, `status` is the reconciler's. Every status carries
//! `observed_revision`, the revision the reconciler last acted on, so a
//! status write (which bumps the revision) does not trigger another round.

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

mod allocation_pool;
mod allowed_address_pair;
mod default_mtu;
mod default_true;
mod deserialize_ip_or_cidr;
mod direction;
mod ether_type;
mod floating_ip;
mod floating_ip_spec;
mod floating_ip_status;
mod host_route;
mod is_false;
mod kinds;
mod network;
mod network_kind;
mod network_spec;
mod network_status;
mod port;
mod port_binding;
mod port_range;
mod port_spec;
mod port_status;
mod protocol;
mod remote;
mod router;
mod router_spec;
mod router_status;
mod rule;
mod security_group;
mod security_group_spec;
mod security_group_status;
mod static_route;
mod subnet;
mod subnet_spec;
mod subnet_status;
mod vpc;
mod vpc_spec;
mod vpc_status;
pub use allocation_pool::AllocationPool;
pub use allowed_address_pair::AllowedAddressPair;
pub use direction::Direction;
pub use ether_type::EtherType;
pub use floating_ip::FloatingIp;
pub use floating_ip_spec::FloatingIpSpec;
pub use floating_ip_status::FloatingIpStatus;
pub use host_route::HostRoute;
pub use kinds::KINDS;
pub use network::Network;
pub use network_kind::NetworkKind;
pub use network_spec::NetworkSpec;
pub use network_status::NetworkStatus;
pub use port::Port;
pub use port_binding::PortBinding;
pub use port_range::PortRange;
pub use port_spec::PortSpec;
pub use port_status::PortStatus;
pub use protocol::Protocol;
pub use remote::Remote;
pub use router::Router;
pub use router_spec::RouterSpec;
pub use router_status::RouterStatus;
pub use rule::Rule;
pub use security_group::SecurityGroup;
pub use security_group_spec::SecurityGroupSpec;
pub use security_group_status::SecurityGroupStatus;
pub use static_route::StaticRoute;
pub use subnet::Subnet;
pub use subnet_spec::SubnetSpec;
pub use subnet_status::SubnetStatus;
pub use vpc::Vpc;
pub use vpc_spec::VpcSpec;
pub use vpc_status::VpcStatus;
