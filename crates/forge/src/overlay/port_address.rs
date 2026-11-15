// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{choose_address, planned_family};
use fabric::{Port, Subnet};

/// Prefer the subnet's family when the port has both. A lone IPv6 address
/// is still a port.
pub(in crate::overlay) fn port_address(port: &Port, subnets: &[Subnet]) -> Option<String> {
    choose_address(port, planned_family(port, subnets))
}
