// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::floating_local::FloatingLocal;
use crate::overlay::types::port_endpoint::PortEndpoint;
use crate::overlay::types::provider_attach::ProviderAttach;
use crate::overlay::types::router_namespace::RouterNamespace;
use serde::Deserialize;
use serde::Serialize;

/// One overlay network as realised on this host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentPlan {
    pub id: String,
    pub vni: u32,
    /// Payload MTU. Never 0. Default is the underlay MTU minus 50.
    pub mtu: u16,
    pub bridge: String,
    /// `vx` plus 12 hex digits, or empty when [`Self::provider`] is set.
    /// An empty name means the agent must not create a VXLAN device.
    pub vxlan: String,
    /// This host's underlay IPv4.
    pub local: String,
    /// Other underlay IPv4s, sorted and unique. Never this host, never
    /// `0.0.0.0`. Empty on a provider segment.
    pub peers: Vec<String>,
    pub ports: Vec<PortEndpoint>,
    /// Namespaces whose first attachment is this network. A router on several
    /// networks appears once and lists every interface.
    pub routers: Vec<RouterNamespace>,
    /// Answer metadata on this bridge even when the router namespace is on
    /// another host. Missing on older plans means false.
    #[serde(default)]
    pub metadata: bool,
    /// Host physnet attachment. Absent on a VXLAN segment and on older plans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderAttach>,
    /// Floating addresses hosted on this segment. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub floating: Vec<FloatingLocal>,
}
