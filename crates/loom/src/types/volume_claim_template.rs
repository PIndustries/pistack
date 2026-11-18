// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::default_volume_type::default_volume_type;

/// A per-ordinal block volume to create in `slab` and attach to the pod.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeClaimTemplate {
    pub name: String,
    pub size_gib: u64,
    #[serde(default = "default_volume_type")]
    pub volume_type: String,
}
