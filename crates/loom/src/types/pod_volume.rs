// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::volume_source::VolumeSource;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodVolume {
    pub name: String,
    pub source: VolumeSource,
}
