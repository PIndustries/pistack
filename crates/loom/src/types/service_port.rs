// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::protocol::Protocol;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServicePort {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub protocol: Protocol,
    pub port: u16,
    /// Container port to send to; defaults to `port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
    /// `NodePort` and `LoadBalancer` only: ask for this node port
    /// (30000-32767) instead of the next free one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_port: Option<u16>,
}
