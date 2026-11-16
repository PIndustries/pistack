// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use serde::{Deserialize, Serialize};

/// OVN on a host as its agent last reported it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChassisView {
    #[serde(default)]
    pub chassis: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub bridge_mappings: String,
}
