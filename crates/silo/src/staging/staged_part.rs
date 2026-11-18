// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StagedPart {
    pub number: u32,
    pub size: u64,
    /// Hex MD5 of the part, its ETag.
    pub md5: String,
    /// When the part was written, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub modified_ms: u64,
}
