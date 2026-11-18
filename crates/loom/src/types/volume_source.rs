// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeSource {
    /// Scratch space that lives as long as the pod.
    EmptyDir,
    /// A `keep` secret, mounted as files.
    Secret(String),
    /// A ConfigMap (name or id), mounted as files.
    ConfigMap(String),
    /// A `slab` block volume, attached to the pod's machine.
    Volume(Id),
}
