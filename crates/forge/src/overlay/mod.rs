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

mod address_on_subnet;
mod all_plans;
mod attach_float_ports;
mod choose_address;
mod classify;
mod collides_with_router_ip;
mod dhcp_leases;
mod enabled;
mod external_for;
mod float_binding;
mod floats_for;
mod follows_port;
mod guest;
mod member_cidrs;
mod on_subnet;
mod placed_guests;
mod planned_family;
mod plans_for_machine;
mod port_address;
mod port_endpoint;
mod real_vni;
mod realise;
mod realised;
mod refused_physnet;
mod router_hosts;
mod routers_on;
mod security_rules;
mod seen_host;
mod segment_for_nic;
mod segment_hosts;
mod service_endpoint_view;
mod service_port_view;
mod service_record;
mod service_rules;
mod service_status_view;
mod service_view;
mod shared_nat;
mod shared_nat_network;
mod span;
mod store_err;

pub(in crate::overlay) use address_on_subnet::address_on_subnet;
pub use all_plans::all_plans;
pub(in crate::overlay) use attach_float_ports::attach_float_ports;
pub(in crate::overlay) use choose_address::choose_address;
pub(in crate::overlay) use classify::classify;
pub(in crate::overlay) use collides_with_router_ip::collides_with_router_ip;
pub(in crate::overlay) use dhcp_leases::dhcp_leases;
pub use enabled::enabled;
pub(in crate::overlay) use external_for::external_for;
pub(in crate::overlay) use float_binding::float_binding;
pub(in crate::overlay) use floats_for::floats_for;
pub(in crate::overlay) use follows_port::follows_port;
pub(in crate::overlay) use guest::Guest;
pub(in crate::overlay) use member_cidrs::member_cidrs;
pub(in crate::overlay) use on_subnet::on_subnet;
pub(in crate::overlay) use placed_guests::placed_guests;
pub(in crate::overlay) use planned_family::planned_family;
pub use plans_for_machine::plans_for_machine;
pub(in crate::overlay) use port_address::port_address;
pub(in crate::overlay) use port_endpoint::port_endpoint;
pub(in crate::overlay) use real_vni::real_vni;
pub(in crate::overlay) use realise::realise;
pub(in crate::overlay) use realised::Realised;
pub(in crate::overlay) use refused_physnet::refused_physnet;
pub(in crate::overlay) use router_hosts::router_hosts;
pub(in crate::overlay) use routers_on::routers_on;
pub(in crate::overlay) use security_rules::security_rules;
pub(in crate::overlay) use seen_host::SeenHost;
pub use segment_for_nic::segment_for_nic;
pub(in crate::overlay) use segment_hosts::segment_hosts;
pub use service_endpoint_view::ServiceEndpointView;
pub use service_port_view::ServicePortView;
pub use service_record::ServiceRecord;
pub(in crate::overlay) use service_rules::service_rules;
pub use service_status_view::ServiceStatusView;
pub use service_view::ServiceView;
pub(in crate::overlay) use shared_nat::shared_nat;
pub(in crate::overlay) use shared_nat_network::shared_nat_network;
pub(in crate::overlay) use span::span;
pub(in crate::overlay) use store_err::store_err;

#[cfg(test)]
mod test_support;
#[cfg(test)]
pub(in crate::overlay) use test_support::{host, placed, plan_for, service, sg_port, tenant};

#[cfg(test)]
mod a_hostnat_network_stays_on_the_host_bridge_tests;
#[cfg(test)]
mod dual_stack_port_plans_rules_for_both_addresses_tests;
#[cfg(test)]
mod floating_ip_follows_the_guest_onto_the_provider_network_tests;
#[cfg(test)]
mod group_members_expand_to_the_other_port_and_an_outsider_is_absent_tests;
#[cfg(test)]
mod ipv6_port_gets_ipv6_rules_and_ipv4_port_keeps_ipv4_rules_tests;
#[cfg(test)]
mod ipv6_router_external_snat_is_a_private_ula_tests;
#[cfg(test)]
mod the_project_router_answers_service_addresses_with_ready_backends_tests;
#[cfg(test)]
mod two_projects_with_the_same_cidr_get_two_segments_and_the_peer_is_not_flooded_tests;
