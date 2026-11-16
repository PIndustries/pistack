// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::dnat_rule::DnatRule;
use crate::overlay::types::router_interface::RouterInterface;
use crate::overlay::types::service_rule::ServiceRule;
use serde::Deserialize;
use serde::Serialize;

/// One router namespace. The gateway address lives on `veth_ns`, not on the bridge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterNamespace {
    pub id: String,
    pub netns: String,
    pub interfaces: Vec<RouterInterface>,
    /// Root end of the external veth. `c` plus 12 hex digits of the router id.
    pub external_veth_root: String,
    /// Namespace end of the external veth. `d` plus the same 12 hex digits.
    pub external_veth_ns: String,
    /// Router address on the external network. Installed as a host prefix
    /// (`/32` or `/128`) in the namespace. IPv6 is unique-local (`fc00::/7`).
    pub external_ip: Option<String>,
    /// Recorded only. The agent must not move, enslave, or address this device.
    /// Empty, and left out of the document, when the router has no external
    /// address: that router gets no external veth.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_dev: String,
    /// True only when [`Self::external_ip`] is set.
    pub snat: bool,
    pub dnat: Vec<DnatRule>,
    pub metadata: bool,
    /// Service addresses answered in this namespace. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceRule>,
}
