// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Id, Phase};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RouterStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Port on the external network, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_port: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ip: Option<IpAddr>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}
