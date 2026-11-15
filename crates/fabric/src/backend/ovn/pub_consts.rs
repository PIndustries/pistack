// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Environment variable naming the northbound database endpoint.
pub const NB_ENV: &str = "PISTACK_OVN_NB";
/// Where a host running the northbound database listens by default.
pub const DEFAULT_NB: &str = "unix:/var/run/ovn/ovnnb_db.sock";
/// The OVS bridge `ovn-controller` manages on every host.
pub const INTEGRATION_BRIDGE: &str = "br-int";
/// Environment variable listing the chassis that may host router gateway
/// ports, highest priority first, comma separated.
pub const GATEWAY_CHASSIS_ENV: &str = "PISTACK_OVN_GATEWAY_CHASSIS";
/// Most chassis one gateway port is scheduled on. OVN keeps a BFD session
/// between every pair, so the list is kept short.
pub const MAX_GATEWAY_CHASSIS: usize = 5;
/// The address guests ask for metadata.
pub const METADATA_ADDRESS: &str = "169.254.169.254";
