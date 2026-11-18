// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Id, Phase};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FloatingIpStatus {
    #[serde(default)]
    pub phase: Phase,
    /// The port's fixed IP the floating address maps to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_ip: Option<IpAddr>,
    /// Router carrying the NAT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<Id>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}
