// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NetworkStatus {
    #[serde(default)]
    pub phase: Phase,
    /// VNI for overlays, VLAN tag for provider networks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<u32>,
    /// `hostnat` or `overlay`. Set once. A network that already has a
    /// host-NAT port stays `hostnat`, so later guests join that bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datapath: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}
