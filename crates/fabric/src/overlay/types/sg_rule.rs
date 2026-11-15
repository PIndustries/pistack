// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use serde::Serialize;

/// One security-group match. `proto` is `tcp`, `udp`, `icmp`, or `any`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SgRule {
    pub proto: String,
    pub port_min: u16,
    pub port_max: u16,
    pub cidr: String,
}
