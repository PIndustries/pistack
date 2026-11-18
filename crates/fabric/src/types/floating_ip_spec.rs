// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FloatingIpSpec {
    /// External network the address is drawn from.
    pub network: Id,
    /// Allocated on create when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<IpAddr>,
    /// Port the address is NATed to, if associated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<Id>,
}
