// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The balancer's request: where its VIP lives and which data plane realises
//! it.

use super::provider::Provider;
use crate::types::default_true::default_true;
use pistack_types::Id;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

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
