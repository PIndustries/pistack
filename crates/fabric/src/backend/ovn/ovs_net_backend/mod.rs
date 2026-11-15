// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod addresses_tests;
#[cfg(test)]
mod allowed_pair_tests;
mod apply_security_group;
mod associate_fip;
#[cfg(test)]
mod associate_fip_tests;
mod attach;
#[cfg(test)]
mod configured_chassis_tests;
mod delete_fip;
mod delete_network;
#[cfg(test)]
mod delete_network_tests;
mod delete_port;
mod delete_router;
#[cfg(test)]
mod delete_router_tests;
mod delete_security_group;
mod delete_subnet;
#[cfg(test)]
mod dhcp_options_tests;
#[cfg(test)]
mod drop_group_tests;
#[cfg(test)]
mod drop_iface_route_tests;
mod ensure_network;
#[cfg(test)]
mod ensure_network_tests;
mod ensure_port;
mod ensure_router;
mod ensure_subnet;
#[cfg(test)]
mod follower_tests;
#[cfg(test)]
mod gateway_without_subnet_tests;
#[cfg(test)]
mod members_tests;
#[cfg(test)]
mod missing_switch_tests;
mod name;
#[cfg(test)]
mod ports_tests;
mod refresh;
#[cfg(test)]
mod rules_tests;
#[cfg(test)]
mod snat_tests;
mod trait_net_backend;
