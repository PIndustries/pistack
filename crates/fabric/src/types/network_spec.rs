// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

use super::default_mtu::default_mtu;
use super::is_false::is_false;
use super::network_kind::NetworkKind;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSpec {
    #[serde(default)]
    pub kind: NetworkKind,
    #[serde(default = "default_mtu")]
    pub mtu: u16,
    /// External networks are where routers get their gateway and floating
    /// IPs are drawn from.
    #[serde(default)]
    pub external: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_domain: Option<String>,
    /// Give every machine that has an IPv4 a floating IP from this network
    /// without being asked. Off unless set. Only an external network can
    /// carry it, and only one network in the cloud at a time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_floating_ip: bool,
    /// The VPC this network belongs to. Set at create and never changed; a
    /// network in a VPC is not external and not a provider network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpc: Option<Id>,
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self {
            kind: NetworkKind::Overlay,
            mtu: default_mtu(),
            external: false,
            dns_domain: None,
            auto_floating_ip: false,
            vpc: None,
        }
    }
}
