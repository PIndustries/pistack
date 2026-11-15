// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use serde::Serialize;

/// One DHCP route handed to a guest with its lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DhcpRoute {
    pub destination: String,
    pub nexthop: String,
}
