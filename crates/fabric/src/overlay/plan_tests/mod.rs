// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(super) mod dhcp_lease_round_trips_onto_the_interface;
pub(super) mod dual_stack_port_carries_its_other_address_and_rules;
pub(super) mod floating_fixed_on_this_segment_is_local;
pub(super) mod ipv6_external_snat_is_a_private_ula;
pub(super) mod ipv6_port_and_gateway_are_canonical;
pub(super) mod loopback_and_public_underlay_are_usable;
pub(super) mod mtu_is_underlay_minus_vxlan_overhead;
pub(super) mod one_router_two_networks_is_one_namespace;
pub(super) mod peers_are_remote_underlays_only;
pub(super) mod plan_json_round_trips;
pub(super) mod planned_names_are_not_hostnat_bridges;
pub(super) mod provider_network_skips_vxlan_and_peers;
pub(super) mod router_legs_merge_their_service_rules;
pub(super) mod router_without_external_ip_sends_no_external_dev;
pub(super) mod router_without_services_leaves_the_field_out;
pub(super) mod same_cidr_is_two_bridges_and_two_namespaces;
pub(super) mod service_rules_are_checked;
pub(super) mod simbr0_physnet_is_refused;
pub(super) mod two_networks_get_distinct_vnis_and_devices;
pub(super) mod unspecified_peer_is_an_error;
pub(super) mod vni_bounds;
