// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::is_false;

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachedDisk {
    pub(in crate::runtime::youki) volume: String,
    pub(in crate::runtime::youki) path: String,
    pub(in crate::runtime::youki) device: String,
    #[serde(default)]
    pub(in crate::runtime::youki) read_only: bool,
    /// The path is a host device this agent mapped from an RBD image.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(in crate::runtime::youki) rbd_mapped: bool,
}
