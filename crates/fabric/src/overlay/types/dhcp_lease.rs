// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::dhcp_route::DhcpRoute;
use serde::Deserialize;
use serde::Serialize;

/// One address handed out on a router leg.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DhcpLease {
    pub mac: String,
    pub address: String,
    pub dns: Vec<String>,
    pub routes: Vec<DhcpRoute>,
}
