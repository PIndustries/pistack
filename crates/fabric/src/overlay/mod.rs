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

mod attach_router;
mod canonical_dhcp;
mod canonical_provider;
mod checks;
mod consts;
mod device_hex;
#[cfg(test)]
mod device_hex_tests;
mod external_ips;
mod merge_router;
mod plan;
mod plan_floating;
mod plan_ports;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod tests;
mod types;

pub use crate::overlay::checks::is_private_ula::is_private_ula;
pub use crate::overlay::consts::PLAN_VERSION;
pub use crate::overlay::consts::VXLAN_PORT;
pub use crate::overlay::device_hex::device_hex;
pub use crate::overlay::plan::plan;
pub use crate::overlay::types::dhcp_lease::DhcpLease;
pub use crate::overlay::types::dhcp_route::DhcpRoute;
pub use crate::overlay::types::dnat_rule::DnatRule;
pub use crate::overlay::types::floating_local::FloatingLocal;
pub use crate::overlay::types::overlay_plan::OverlayPlan;
pub use crate::overlay::types::plan_error::PlanError;
pub use crate::overlay::types::plan_float::PlanFloat;
pub use crate::overlay::types::plan_host::PlanHost;
pub use crate::overlay::types::plan_network::PlanNetwork;
pub use crate::overlay::types::plan_port::PlanPort;
pub use crate::overlay::types::plan_provider::PlanProvider;
pub use crate::overlay::types::plan_router::PlanRouter;
pub use crate::overlay::types::port_address::PortAddress;
pub use crate::overlay::types::port_endpoint::PortEndpoint;
pub use crate::overlay::types::provider_attach::ProviderAttach;
pub use crate::overlay::types::router_interface::RouterInterface;
pub use crate::overlay::types::router_namespace::RouterNamespace;
pub use crate::overlay::types::segment_plan::SegmentPlan;
pub use crate::overlay::types::service_backend::ServiceBackend;
pub use crate::overlay::types::service_rule::ServiceRule;
pub use crate::overlay::types::sg_rule::SgRule;
