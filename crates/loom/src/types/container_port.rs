// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::protocol::Protocol;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ContainerPort {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub container_port: u16,
    #[serde(default)]
    pub protocol: Protocol,
}
