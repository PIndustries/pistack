// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The balancer: a VIP, and which data plane realises it.

use pistack_types::{kind, Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Which data plane realises the balancer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// L4 TCP and UDP. The in-process forwarder serves this value. The default.
    /// Older rows and documents that say `ovn` read as this value.
    #[default]
    #[serde(alias = "ovn")]
    Forward,
    /// HTTP. The in-process reverse proxy serves this value.
    Relay,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BalancerSpec {
    /// Network (id or name) the VIP lives on.
    pub vip_network: String,
    /// Explicit VIP; allocated by the provider when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip_address: Option<IpAddr>,
    /// Bind this exact port on the VIP instead of an ephemeral one. Used
    /// for service node ports; a port already in use stays unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip_port: Option<u16>,
    /// Floating IP (fabric id) to publish the VIP on, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating_ip: Option<Id>,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default = "default_true")]
    pub admin_up: bool,
}

pub(super) fn default_true() -> bool {
    true
}

impl Default for BalancerSpec {
    fn default() -> Self {
        Self {
            vip_network: String::new(),
            vip_address: None,
            vip_port: None,
            floating_ip: None,
            provider: Provider::Forward,
            admin_up: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BalancerStatus {
    #[serde(default)]
    pub phase: Phase,
    /// The VIP as realised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip: Option<IpAddr>,
    /// Provider-side handle (OVN `Load_Balancer` uuid, Pingora listener id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// A load balancer: one VIP, many listeners.
    Balancer,
    BalancerSpec,
    BalancerStatus,
    kind::BALANCER
);
