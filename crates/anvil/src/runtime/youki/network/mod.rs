// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[path = "observed_net.rs"]
mod observed_net;
pub(in crate::runtime::youki) use observed_net::*;
#[path = "drop_link.rs"]
mod drop_link;
pub(in crate::runtime::youki) use drop_link::*;
#[path = "host_netns_from.rs"]
mod host_netns_from;
pub(in crate::runtime::youki) use host_netns_from::*;
#[path = "ip_privilege.rs"]
mod ip_privilege;
pub(in crate::runtime::youki) use ip_privilege::*;
#[path = "joined_netns.rs"]
mod joined_netns;
pub(in crate::runtime::youki) use joined_netns::*;
#[path = "joins_new_net.rs"]
mod joins_new_net;
pub(in crate::runtime::youki) use joins_new_net::*;
#[path = "nic_addresses.rs"]
mod nic_addresses;
pub(in crate::runtime::youki) use nic_addresses::*;
#[path = "observe.rs"]
mod observe;
pub(in crate::runtime::youki) use observe::*;
#[path = "provision_network.rs"]
mod provision_network;
pub(in crate::runtime::youki) use provision_network::*;
#[path = "release_recorded.rs"]
mod release_recorded;
pub(in crate::runtime::youki) use release_recorded::*;
#[path = "requested_host_netns.rs"]
mod requested_host_netns;
pub(in crate::runtime::youki) use requested_host_netns::*;
#[path = "running_from.rs"]
mod running_from;
pub(in crate::runtime::youki) use running_from::*;

#[cfg(test)]
#[path = "a_bad_mac_is_rejected_before_ip_tests.rs"]
mod a_bad_mac_is_rejected_before_ip_tests;

#[cfg(test)]
#[path = "a_pod_without_a_port_does_not_call_ip_tests.rs"]
mod a_pod_without_a_port_does_not_call_ip_tests;

#[cfg(test)]
#[path = "explicit_netns_skips_the_veth_tests.rs"]
mod explicit_netns_skips_the_veth_tests;

#[cfg(test)]
#[path = "failed_create_removes_the_namespace_tests.rs"]
mod failed_create_removes_the_namespace_tests;

#[cfg(test)]
#[path = "pod_joins_its_bridge_and_teardown_removes_the_namespace_tests.rs"]
mod pod_joins_its_bridge_and_teardown_removes_the_namespace_tests;

#[cfg(test)]
#[path = "rootless_agent_reports_that_the_pod_has_no_network_tests.rs"]
mod rootless_agent_reports_that_the_pod_has_no_network_tests;

#[cfg(test)]
#[path = "sudo_prefixes_ip_when_the_agent_is_not_root_tests.rs"]
mod sudo_prefixes_ip_when_the_agent_is_not_root_tests;
