// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::sg_rule::SgRule;
use serde::Deserialize;
use serde::Serialize;

/// Another fixed address on a port and the security-group rules for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortAddress {
    /// No prefix. IPv4 or IPv6.
    pub address: String,
    #[serde(default)]
    pub ingress: Vec<SgRule>,
    #[serde(default)]
    pub egress: Vec<SgRule>,
}
