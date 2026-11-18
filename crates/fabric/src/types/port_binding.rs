// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the backend reports once a port is realised.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PortBinding {
    /// `ovs`, `mock`, ...
    pub vif_type: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vif_details: BTreeMap<String, String>,
}
